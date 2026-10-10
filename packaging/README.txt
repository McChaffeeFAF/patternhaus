Patternhaus (pre-alpha), an open-source pattern-based DAW with an agent interface
https://github.com/McChaffeeFAF/patternhaus

Run the app
  Windows:      Patternhaus.bat [project_dir]
  Linux/macOS:  ./patternhaus.sh [project_dir]
  or directly:  ph-app [project_dir] [--buffer N] [--silent] [--demo-agent]
  A project is a folder with a project.json; a new folder is fine.
  macOS: the binaries are not signed; allow them in System Settings > Privacy & Security.

Let an agent drive it (MCP)
  patternhaus-cli mcp --headless --project <dir>     an MCP server over stdio, no audio device
  patternhaus-cli mcp --connect                      talk to a running ph-app
  For Claude Code: claude mcp add patternhaus -- <path>/patternhaus-cli mcp --connect
  patternhaus-cli --help lists the other commands (run, render, analyze, ...).

Files
  assets/                  factory drum kit and Haus Synth presets (self-generated, see assets/PROVENANCE.toml)
  THIRD_PARTY_NOTICES.md   licences of the open-source components
  LICENSE-MIT, LICENSE-APACHE
  COMMIT.txt               the source commit of this build

Licence: MIT OR Apache-2.0. A build marked "asio" is GPLv3 as a whole (see GPL-NOTICE.txt).
FL Studio is a trademark of Image-Line; Patternhaus is not affiliated with Image-Line.
