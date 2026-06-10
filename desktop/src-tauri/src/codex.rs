use crate::config::AppConfig;

pub struct CodexSnippet {
    pub stream_url: String,
    pub cli_override: String,
    pub toml_snippet: String,
}

pub fn build_codex_snippet(config: &AppConfig, user_token: Option<&str>) -> CodexSnippet {
    let stream_url = match user_token {
        Some(token) => format!(
            "{}?userToken={}",
            config.mcp_stream_url(),
            urlencoding::encode(token)
        ),
        None => config.mcp_stream_url(),
    };

    let cli_override = format!(r#"mcp_servers.penpot.url="{stream_url}""#);

    let toml_snippet = format!(
        r#"# Penpot MCP — paste into Codex config or pass via: codex -c '...'
[mcp_servers.penpot]
url = "{stream_url}"
"#
    );

    CodexSnippet {
        stream_url,
        cli_override,
        toml_snippet,
    }
}
