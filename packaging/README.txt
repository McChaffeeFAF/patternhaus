Beast Studio (pre-alpha), an open-source pattern-based DAW with an agent interface
https://github.com/McChaffeeFAF/beast-studio

Run the app
  Windows:      "Beast Studio.bat" [project_dir]
  Linux/macOS:  ./beast-studio.sh [project_dir]
  or directly:  beast-studio [project_dir] [--buffer N] [--silent] [--demo-agent]
  A project is a folder with a project.json; a new folder is fine.
  macOS: the binaries are not signed; allow them in System Settings > Privacy & Security.

Let an agent drive it (MCP)
  beast-studio-cli mcp --headless --project <dir>     an MCP server over stdio, no audio device
  beast-studio-cli mcp --connect                      talk to a running Beast Studio
  For Claude Code: claude mcp add beast-studio -- <path>/beast-studio-cli mcp --connect
  beast-studio-cli --help lists the other commands (run, render, analyze, ...).

Files
  assets/                  factory drum kit and Haus Synth presets (self-generated, see assets/PROVENANCE.toml)
  THIRD_PARTY_NOTICES.md   licences of the open-source components
  LICENSE-MIT, LICENSE-APACHE
  COMMIT.txt               the source commit of this build

Licence: MIT OR Apache-2.0. A build marked "asio" is GPLv3 as a whole (see GPL-NOTICE.txt).
FL Studio is a trademark of Image-Line; Beast Studio is not affiliated with Image-Line.
