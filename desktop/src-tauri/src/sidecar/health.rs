use reqwest::Client;

pub struct HealthClient {
    inner: Client,
}

impl HealthClient {
    pub fn new() -> Result<Self, reqwest::Error> {
        Client::builder()
            .timeout(std::time::Duration::from_secs(8))
            .danger_accept_invalid_certs(true)
            .build()
            .map(|inner| Self { inner })
    }

    pub async fn probe_url(&self, url: &str) -> ProbeResult {
        match self.inner.get(url).send().await {
            Ok(resp) => {
                let status = resp.status().as_u16();
                ProbeResult {
                    reachable: status < 500,
                    status: Some(status),
                    error: None,
                }
            }
            Err(err) => ProbeResult {
                reachable: false,
                status: None,
                error: Some(err.to_string()),
            },
        }
    }

    pub async fn wait_for_reachable(
        &self,
        url: &str,
        timeout: std::time::Duration,
        interval: std::time::Duration,
    ) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            if self.probe_url(url).await.reachable {
                return true;
            }
            tokio::time::sleep(interval).await;
        }
        false
    }
}

#[derive(Debug, Clone)]
pub struct ProbeResult {
    pub reachable: bool,
    pub status: Option<u16>,
    pub error: Option<String>,
}
