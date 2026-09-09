# Foreign configuration discovery evidence

Run `34262041450` tested source `d1428b83df` using actual filesystem fixtures
for Claude context, Cline rules, Codex skills, Cursor rules, Gemini context,
GitHub rules, VS Code MCP configuration, and Windsurf rules. All eight were
discovered. The unchanged inventory test failed with raw exit 100 after the
GitHub provider was disabled, then passed again after source restoration.

The complete driver suite passed 194 tests with zero skips. The complete envd
suite passed 873 tests with two skips. Both paired doctest commands exited zero.
These results apply to the recorded source; combining it with newer fixes
requires another combined run.

The fixture proves source discovery and detects a missing GitHub provider.
It does not prove a real model follows matching instructions, ignores
nonmatching instructions, or behaves correctly across every conflicting source.
Those application and model-directed checks remain necessary for issue #27.
