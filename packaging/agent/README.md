# Connecting a model to Vernon

> **Placeholder.** The agent tools (`vernon-cli mcp`) and the in-app agent
> panel are built on separate branches. When they are merged, the packaging
> script copies their final documents into this folder automatically (from
> `docs/agent/` and `docs/MCP.md` / `docs/AGENT.md` in the source tree) and
> the list below is replaced by the real instructions. Until then treat the
> sections below as the plan, not as working commands.

## Local models (nothing leaves your machine)

Any server that speaks the OpenAI-compatible HTTP API can be used.

| Server | Default address | Start |
|---|---|---|
| Ollama | `http://localhost:11434/v1` | `ollama serve`, then `ollama pull <model>` |
| LM Studio | `http://localhost:1234/v1` | Developer tab > Start Server |
| llama.cpp | `http://localhost:8080/v1` | `llama-server -m model.gguf` |

Pick a model that supports tool calling; small models without it can chat
but cannot drive the node graph.

## Cloud models

Supply your own API key (kept in your user settings, never in a project
file). Prompts and any attached report data are sent to the provider you
choose.

## MCP clients (Claude Code, Cursor, Cline, ...)

Once `vernon-cli mcp` exists, an MCP client starts it as a stdio server. The
generic configuration looks like this (adjust the path to your install):

```json
{
  "mcpServers": {
    "vernon": {
      "command": "C:\\Users\\<you>\\AppData\\Local\\Programs\\Vernon\\bin\\vernon-cli.exe",
      "args": ["mcp"]
    }
  }
}
```

- Claude Code: `claude mcp add vernon -- "<path>\vernon-cli.exe" mcp`
- Cursor: `.cursor/mcp.json` or Settings > MCP
- Cline: MCP Servers > Configure

If `vernon-cli mcp` reports an unknown command, this build predates the agent
tools.
