//! The MCP server (DESIGN §5), on `rmcp`.
//!
//! One [`PatternhausServer`] serves one connection. The client's name (from `clientInfo`)
//! becomes `Caller::agent(<name>)`; its grant comes from [`Grants`]. Tools are the fixed
//! toolset of [`Agent`]. Live events reach a client three ways, so even a minimal client sees
//! them:
//! - `notifications/resources/updated` for `patternhaus://events` (after `resources/subscribe`,
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

pub const EVENTS_URI: &str = "patternhaus://events";
pub const OVERVIEW_URI: &str = "patternhaus://project/overview";

const INSTRUCTIONS: &str = "Patternhaus is a DAW you drive through commands. Start with get_state (overview), then list_commands / describe_command / run. \
Times are ticks: 960 per quarter note, 240 per 16th step; note 60 = C4. Build with run_batch (one undo step), then HEAR it: render + analyze (LUFS, spectrum, key, chords, clashes) \
and SEE it: snapshot (piano_roll, playlist, waveform, spectrogram). Every change returns a musical diff. Factory samples are factory:drums/haus808/kick etc.; synth presets are listed by node_params. \
Other authors' edits are reported in while_you_worked blocks and in the patternhaus://events resource.";

pub struct PatternhausServer {
    agent: Arc<Agent>,
    hub: Arc<EventHub>,
    client: Mutex<String>,
    subscribed: Arc<Mutex<HashSet<String>>>,
    log_events: Arc<AtomicBool>,
    forwarder: AtomicBool,
}

impl PatternhausServer {
    pub fn new(agent: Arc<Agent>) -> Result<Self, String> {
        let hub = agent.studio().events()?;
        Ok(Self {
            agent,
            hub,
            client: Mutex::new("unknown".into()),
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
            }
        }
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
                if subs.lock().unwrap().contains(EVENTS_URI)
                    && peer
                        .notify_resource_updated(ResourceUpdatedNotificationParam::new(EVENTS_URI))
                        .await
                        .is_err()
                {
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
                            .with_logger("patternhaus"),
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
            "patternhaus",
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
        Ok(ListResourcesResult {
            resources: vec![overview, events],
            ..Default::default()
        })
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, McpError> {
        let client = self.note_client(&context);
        let args = match request.uri.as_str() {
            OVERVIEW_URI => json!({ "view": "overview" }),
            EVENTS_URI => json!({ "view": "events" }),
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
            .filter(|u| u.starts_with("patternhaus://"))
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
        loop {
            tokio::select! {
                _ = context.cancelled() => break,
                ev = rx.recv() => match ev {
                    Some(e) if e.author.as_deref() != Some(me.as_str()) => {
                        if context.sink().notify_resource_updated(EVENTS_URI).await.is_err() {
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
