# M2 manual test: agent abilities (T9)

For the owner, with a real MCP client (Claude Desktop, Claude Code) connected to the running app. Nothing here needs an audio device beyond normal playback.

1. **Preview mode.** Ask the agent to "propose a bass line for the open pattern" (it uses `propose`).
   - A strip appears at the top of the Piano roll and Playlist: the agent's name, the title, **Accept** and **Reject**. A toast announces it.
   - The new notes show as dashed ghosts in the agent's colour; a proposed clip shows as a dashed box in the playlist. The project has not changed (the undo history is unchanged).
   - **Reject:** the ghosts vanish, nothing changed, and the agent can read why (`proposal_status`).
   - **Accept:** the notes land, the History panel shows one entry by the agent, and one "Undo all" for that agent (or the agent's `undo_mine`) removes it.
2. **Stale.** Ask for a proposal that changes an existing note, then change that note yourself before deciding. The strip says it is stale and offers only **Dismiss**.
3. **Audio for the agent.** Ask the agent to listen to the chorus (`render` with `listen: true`). An audio-capable client receives a WAV of up to 15 seconds.
4. **Reference.** Put a reference WAV in your project's `samples` folder and ask the agent to compare the mix to it. Check the band differences make sense when you listen to both.
5. **Audio, automation, plugins.** Ask for an audio loop on track 2 fitted to the tempo and a filter sweep. `get_state` views `audio`, `automation` and `plugins` should read back what you see.
