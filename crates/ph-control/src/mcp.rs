//! The MCP server (DESIGN §5), on `rmcp`.
//!
//! One [`PatternhausServer`] serves one connection. The client's name (from `clientInfo`)
//! becomes `Caller::agent(<name>)`; its grant comes from [`Grants`]. Tools are the fixed
//! toolset of [`Agent`]. Live events reach a client three ways, so even a minimal client sees
//! them:
//! - `notifications/resources/updated` for `beast-studio://events` (after `resources/subscribe`,
//!   or a `subscriptions/listen` that names it);
//! - `notifications/message` (logging) once the client has called `logging/setLevel`;
//! - a `while_you_worked` block appended to tool results, and `get_state { view: "events" }`.

use crate::agent::event_line;
use crate::agent::{Agent, Block, ToolOutput};
use crate::events::{Event, EventHub};
use base64::Engine as _;
use rmcp::model::*;
use rmcp::service::{Peer, RequestContext, SubscriptionContext};
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// The URI scheme of the server's resources.
pub const SCHEME: &str = "beast-studio://";
/// The scheme before the rename (2026-11-27): reading, and subscribing to, a resource under it
/// still works and answers as the current scheme does.
pub const LEGACY_SCHEME: &str = "patternhaus://";

pub const EVENTS_URI: &str = "beast-studio://events";
pub const OVERVIEW_URI: &str = "beast-studio://project/overview";
pub const CATALOGUE_URI: &str = "beast-studio://catalogue";
/// One Beast Sampler instrument's import report: `beast-studio://instrument/<id>/report`.
pub const INSTRUMENT_REPORT_TEMPLATE: &str = "beast-studio://instrument/{id}/report";

/// `uri` under the current scheme: a `patternhaus://` URI is its `beast-studio://` twin, any
/// other URI is returned as it is.
pub fn canonical_uri(uri: &str) -> std::borrow::Cow<'_, str> {
    match uri.strip_prefix(LEGACY_SCHEME) {
        Some(rest) => std::borrow::Cow::Owned(format!("{SCHEME}{rest}")),
        None => std::borrow::Cow::Borrowed(uri),
    }
}

/// Whether `uri` names the live-events resource, under either scheme.
fn is_events_uri(uri: &str) -> bool {
    canonical_uri(uri) == EVENTS_URI
}

/// The instrument id of a `beast-studio://instrument/<id>/report` URI (the `patternhaus://`
/// spelling is accepted too).
pub fn instrument_report_id(uri: &str) -> Option<u64> {
    let uri = canonical_uri(uri);
    uri.strip_prefix("beast-studio://instrument/")?
        .strip_suffix("/report")?
        .parse()
        .ok()
}

const INSTRUCTIONS: &str = "Beast Studio is a DAW you drive through commands. Start with get_state (overview), then list_commands / describe_command / run. \
Times are ticks: 960 per quarter note, 240 per 16th step; note 60 = C4. Build with run_batch (one undo step), then HEAR it: render + analyze (LUFS, spectrum, key, chords, clashes, tempo and beat grid) \
and SEE it: snapshot (piano_roll, playlist, waveform, spectrogram). Every change returns a musical diff. Factory samples are factory:drums/haus808/kick etc.; synth presets are listed by node_params. \
Audio clips, automation and plugins are commands too: add_audio_clip, fit_to_tempo, add_automation, add_plugin_instrument; get_state views audio, automation, plugins. Hosted instruments the owner set up (e.g. sampled libraries in their own plugins) are in the plugin catalogue: catalogue.search / catalogue.get (with a playing guide: range, keyswitches, CCs, velocity), catalogue.add_to_channel, then write parts within the guide; catalogue.probe proposes a range and keyswitches when the guide is empty (beast-studio://catalogue lists them). Beast Sampler instruments (SFZ, SF2/SF3, DecentSampler, EXS24 or native): sampler.import or sampler.load puts one on a channel; read get_state view zones (its playing guide: range, keyswitches, velocity layers, gaps; then the zones) or snapshot zone_map, write parts inside the range with a keyswitch note before each articulation change, edit with sampler.zone_move / zone_set / keyswitch_set (each diff names the zones it moved), then render + analyze. beast-studio://instrument/<id>/report holds the import report (what was approximated or skipped); sampler.prune removes instruments no channel plays. For a bigger change use propose: the person previews it as ghost notes and accepts or rejects (proposal_status reads the decision). Dynamics for sampled orchestral instruments usually live on CC1 (mod wheel) or CC11 (expression): write them with cc.shape (a swell, ramp or hold per range); the catalogue guide's CC list says which controllers an instrument uses. render with listen: true returns a short audio excerpt; analyze what [\"reference\"] compares the mix to a reference track; analyze with sample or clip reads imported audio (tempo, first beat, chords on its own grid) without rendering the project; chords resolution half_bar or beat marks changes inside a bar; render format ogg writes a small lossy preview. \nOther authors' edits are reported in while_you_worked blocks and in the beast-studio://events resource.";

pub struct PatternhausServer {
    agent: Arc<Agent>,
    hub: Arc<EventHub>,
    client: Mutex<String>,
    conn: crate::connections::ConnGuard,
    subscribed: Arc<Mutex<HashSet<String>>>,
    log_events: Arc<AtomicBool>,
    forwarder: AtomicBool,
}

impl PatternhausServer {
    pub fn new(agent: Arc<Agent>) -> Result<Self, String> {
        let hub = agent.studio().events()?;
        let conn = agent
            .registry()
            .open(crate::connections::Protocol::Mcp, "unknown");
        Ok(Self {
            agent,
            hub,
            client: Mutex::new("unknown".into()),
            conn,
            subscribed: Arc::default(),
            log_events: Arc::default(),
            forwarder: AtomicBool::new(false),
        })
    }

    fn note_client(&self, ctx: &RequestContext<RoleServer>) -> String {
        if let Some(info) = ctx.client_info() {
            let name = info.name.trim();
            if !name.is_empty() {
                *self.client.lock().unwrap() = name.to_string();
                self.conn.set_client(name);
            }
        }
        self.conn.touch();
        let name = self.client.lock().unwrap().clone();
        self.start_forwarder(ctx.peer.clone(), &name);
        name
    }

    /// Pushes events from other authors to this connection (see the module docs).
    fn start_forwarder(&self, peer: Peer<RoleServer>, client: &str) {
        if self.forwarder.swap(true, Ordering::SeqCst) {
            return;
        }
        let mut rx = self.hub.subscribe();
        let (subs, log) = (self.subscribed.clone(), self.log_events.clone());
        let me = ph_commands::Author::Agent(client.to_string()).to_string();
        tokio::spawn(async move {
            while let Some(ev) = rx.recv().await {
                if ev.author.as_deref() == Some(me.as_str()) {
                    continue;
                }
                // answer on whichever spelling the client subscribed with
                let subscribed: Vec<String> = subs
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|u| is_events_uri(u))
                    .cloned()
                    .collect();
                let mut gone = false;
                for uri in subscribed {
                    if peer
                        .notify_resource_updated(ResourceUpdatedNotificationParam::new(uri))
                        .await
                        .is_err()
                    {
                        gone = true;
                        break;
                    }
                }
                if gone {
                    break;
                }
                if log.load(Ordering::Relaxed) {
                    #[allow(deprecated)]
                    let sent = peer
                        .notify_logging_message(
                            LoggingMessageNotificationParam::new(
                                LoggingLevel::Info,
                                event_json(&ev),
                            )
                            .with_logger("beast-studio"),
                        )
                        .await;
                    if sent.is_err() {
                        break;
                    }
                }
            }
        });
    }
}

fn event_json(e: &Event) -> Value {
    json!({ "seq": e.seq, "kind": e.kind, "author": e.author, "summary": e.summary, "line": event_line(e), "data": e.data })
}

fn to_result(out: ToolOutput) -> CallToolResult {
    let content: Vec<ContentBlock> = out
        .blocks
        .into_iter()
        .map(|b| match b {
            Block::Text(t) => ContentBlock::text(t),
            Block::Audio(wav) => ContentBlock::audio(
                base64::engine::general_purpose::STANDARD.encode(wav),
                "audio/wav",
            ),
            Block::Image(png) => ContentBlock::image(
                base64::engine::general_purpose::STANDARD.encode(png),
                "image/png",
            ),
        })
        .collect();
    if out.is_error {
        CallToolResult::error(content)
    } else {
        CallToolResult::success(content)
    }
}

impl ServerHandler for PatternhausServer {
    #[allow(deprecated)]
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .enable_resources_subscribe()
                .enable_logging()
                .build(),
        )
        .with_server_info(Implementation::new(
            "beast-studio",
            env!("CARGO_PKG_VERSION"),
        ))
        .with_instructions(INSTRUCTIONS)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        self.note_client(&context);
        let tools = self
            .agent
            .tools()
            .into_iter()
            .map(|t| {
                let mut schema = match t.input_schema {
                    Value::Object(m) => m,
                    _ => serde_json::Map::new(),
                };
                schema
                    .entry("type")
                    .or_insert_with(|| Value::String("object".into()));
                Tool::new(t.name, t.description, Arc::new(schema))
            })
            .collect();
        Ok(ListToolsResult {
            tools,
            ..Default::default()
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let client = self.note_client(&context);
        self.conn.count_command();
        let agent = self.agent.clone();
        let name = request.name.to_string();
        let args = request.arguments.map_or(json!({}), Value::Object);
        let out = tokio::task::spawn_blocking(move || agent.call(&client, &name, args))
            .await
            .map_err(|e| McpError::internal_error(format!("tool task failed: {e}"), None))?;
        Ok(to_result(out).into())
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        self.note_client(&context);
        let mut overview = Resource::new(OVERVIEW_URI, "Project overview");
        overview.description =
            Some("Tempo, channels, patterns, key, chords, sections and warnings.".into());
        overview.mime_type = Some("application/json".into());
        let mut events = Resource::new(EVENTS_URI, "Live events");
        events.description = Some(
            "Recent edits by any author, transport changes and overloads. Subscribe to be notified."
                .into(),
        );
        events.mime_type = Some("application/json".into());
        let mut catalogue = Resource::new(CATALOGUE_URI, "Plugin instrument catalogue");
        catalogue.description = Some(
            "Saved plugin instruments with tags, playing-guide summaries and whether each plugin is installed."
                .into(),
        );
        catalogue.mime_type = Some("application/json".into());
        let mut resources = vec![overview, events, catalogue];
        for (id, name) in self.agent.instrument_names() {
            let uri = INSTRUMENT_REPORT_TEMPLATE.replace("{id}", &id.to_string());
            let mut r = Resource::new(uri, format!("Import report: {name}"));
            r.description = Some(format!(
                "Beast Sampler instrument {name}: its source, groups and zones, playing guide, and what the import approximated or skipped."
            ));
            r.mime_type = Some("application/json".into());
            resources.push(r);
        }
        Ok(ListResourcesResult {
            resources,
            ..Default::default()
        })
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, McpError> {
        self.note_client(&context);
        let t = ResourceTemplate::new(INSTRUMENT_REPORT_TEMPLATE, "Instrument import report")
            .with_description(
                "A Beast Sampler instrument's import report and summary; get_state view zones lists the ids.",
            )
            .with_mime_type("application/json");
        Ok(ListResourceTemplatesResult {
            resource_templates: vec![t],
            ..Default::default()
        })
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, McpError> {
        let client = self.note_client(&context);
        let uri = canonical_uri(&request.uri).into_owned();
        let args = match uri.as_str() {
            OVERVIEW_URI => json!({ "view": "overview" }),
            EVENTS_URI => json!({ "view": "events" }),
            CATALOGUE_URI => json!({ "view": "catalogue" }),
            other if instrument_report_id(other).is_some() => json!({
                "view": "report",
                "args": { "instrument": instrument_report_id(other) },
            }),
            other => {
                return Err(McpError::invalid_params(
                    format!("unknown resource {other}"),
                    None,
                ));
            }
        };
        let agent = self.agent.clone();
        let out = tokio::task::spawn_blocking(move || agent.call(&client, "get_state", args))
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let text = out
            .blocks
            .into_iter()
            .find_map(|b| match b {
                Block::Text(t) => Some(t),
                _ => None,
            })
            .unwrap_or_default();
        let mut c = ResourceContents::text(text, request.uri);
        if let ResourceContents::TextResourceContents { mime_type, .. } = &mut c {
            *mime_type = Some("application/json".into());
        }
        Ok(ReadResourceResult::new(vec![c]).into())
    }

    #[allow(deprecated)]
    async fn subscribe(
        &self,
        request: SubscribeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<(), McpError> {
        self.note_client(&context);
        self.subscribed.lock().unwrap().insert(request.uri);
        Ok(())
    }

    #[allow(deprecated)]
    async fn unsubscribe(
        &self,
        request: UnsubscribeRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<(), McpError> {
        self.subscribed.lock().unwrap().remove(&request.uri);
        Ok(())
    }

    #[allow(deprecated)]
    async fn set_level(
        &self,
        _request: SetLevelRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<(), McpError> {
        self.note_client(&context);
        self.log_events.store(true, Ordering::Relaxed);
        Ok(())
    }

    fn accepted_subscription_filter(
        &self,
        requested: &SubscriptionFilter,
    ) -> Option<SubscriptionFilter> {
        let uris: Vec<String> = requested
            .resource_subscriptions
            .iter()
            .flatten()
            .filter(|u| u.starts_with(SCHEME) || u.starts_with(LEGACY_SCHEME))
            .cloned()
            .collect();
        (!uris.is_empty()).then(|| {
            SubscriptionFilter::builder()
                .resource_subscriptions(uris)
                .build()
        })
    }

    async fn listen(&self, context: SubscriptionContext) -> Result<(), McpError> {
        let mut rx = self.hub.subscribe();
        let me = ph_commands::Author::Agent(self.client.lock().unwrap().clone()).to_string();
        let events: Vec<String> = context
            .accepted()
            .resource_subscriptions
            .iter()
            .flatten()
            .filter(|u| is_events_uri(u))
            .cloned()
            .collect();
        loop {
            tokio::select! {
                _ = context.cancelled() => break,
                ev = rx.recv() => match ev {
                    Some(e) if e.author.as_deref() != Some(me.as_str()) => {
                        let mut gone = false;
                        for uri in &events {
                            if context.sink().notify_resource_updated(uri.clone()).await.is_err() {
                                gone = true;
                                break;
                            }
                        }
                        if gone {
                            break;
                        }
                    }
                    Some(_) => {}
                    None => break,
                },
            }
        }
        Ok(())
    }
}
