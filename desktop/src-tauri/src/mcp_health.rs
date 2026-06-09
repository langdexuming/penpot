use crate::config::AppConfig;

#[derive(Debug, serde::Serialize)]
pub struct McpHealthReport {
    pub penpot_uri: String,
    pub mcp_stream_url: String,
    pub mcp_ws_url: String,
    pub stream_reachable: bool,
    pub stream_status: Option<u16>,
    pub error: Option<String>,
}

pub async fn check_mcp_health(config: &AppConfig) -> McpHealthReport {
    let stream_url = config.mcp_stream_url();
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(8))
        .danger_accept_invalid_certs(true)
        .build();

    let mut report = McpHealthReport {
        penpot_uri: config.normalized_public_uri(),
        mcp_stream_url: stream_url.clone(),
        mcp_ws_url: config.mcp_ws_url(),
        stream_reachable: false,
        stream_status: None,
        error: None,
    };

    let client = match client {
        Ok(c) => c,
        Err(e) => {
            report.error = Some(e.to_string());
            return report;
        }
    };

    match client.get(&stream_url).send().await {
        Ok(resp) => {
            report.stream_status = Some(resp.status().as_u16());
            report.stream_reachable = resp.status().is_success() || resp.status().as_u16() == 405;
        }
        Err(e) => {
            report.error = Some(e.to_string());
        }
    }

    report
}
