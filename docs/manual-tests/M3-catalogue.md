# M3 manual test: plugin instrument catalogue (T9)

For the owner, with the app running and a real MCP client (Claude Desktop, Claude Code)
connected. It needs Kontakt 7 (or Kontakt Player) and one of your licensed libraries; any other
CLAP or VST3 instrument works the same way. The catalogue folder is
`%APPDATA%\Patternhaus\catalogue` (see `docs/formats/catalogue.md`).

1. **Load the instrument.** Scan plugins if Kontakt is not listed yet (`+ Channel` > plugins >
   Scan). Add Kontakt as a channel, open its editor from the channel's window, and load a
   library instrument (for example a strings patch). Play a few notes to hear it.
2. **Save it.** Right-click the channel in the channel rack > **Save to catalogue…**. Give it a
   name, tags (e.g. `strings, kontakt, <library>`) and a short description, then **Save**.
   - The browser's **Catalogue** tab lists it, with its tags.
   - `%APPDATA%\Patternhaus\catalogue\<name>\` holds `entry.toml` and `plugin.state`.
3. **Guide.** Right-click the entry in the Catalogue tab > **Edit playing guide…**. Set the range
   from the library's documentation, the keyswitches it uses and the CCs (CC1 dynamics, CC11
   expression), then **Save guide**. Clicking the entry shows the guide under the list.
4. **Probe (agent).** Ask the agent to "probe the <name> catalogue instrument, keyswitch
   candidates C0 to B0" (`catalogue.probe` with `keyswitches`). It works without the editor open.
   Compare its proposed range and keyswitches with the library's documentation; it may miss
   keyswitches that select the articulation already playing. Raise `gap_ms` for long releases.
5. **Agent plays it.** Start a new project (or a new pattern) and ask the agent: "add the <name>
   instrument from the catalogue and write an eight-bar part for it". It should:
   - find it (`catalogue.search` or the `patternhaus://catalogue` resource);
   - add it (`catalogue.add_to_channel`): a new channel that sounds exactly like the one you
     saved, without opening Kontakt's editor;
   - read the guide (in the add result, `catalogue.get`, or `get_state { view: "plugins" }`) and
     keep the notes inside the range, using keyswitches and CCs as the guide says;
   - render and analyze the result. Listen: the part plays the library you saved.
6. **Robustness.** Save the project and reopen it: the channel restores (the project has its
   own copy in `plugins/`). Then edit the entry's `entry.toml` by hand (e.g. the description)
   and check the Catalogue tab shows the change. After a Kontakt update, adding the entry still
   works and reports the version change as a warning.
