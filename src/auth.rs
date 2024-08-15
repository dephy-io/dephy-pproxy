use serde::Deserialize;

pub struct AuthClient {
    local_id: libp2p::PeerId,
    endpoint: reqwest::Url,
    client: reqwest::Client,
}

#[derive(Deserialize)]
struct AuthClientResponse {
    data: bool,
}

impl AuthClient {
    pub fn new(local_id: libp2p::PeerId, endpoint: reqwest::Url) -> AuthClient {
        AuthClient {
            local_id,
            endpoint,
            client: reqwest::Client::new(),
        }
    }

    pub async fn is_valid(&self, token: &str) -> Result<bool, reqwest::Error> {
        let url = self.endpoint.join("access-control").unwrap();
        let params = [
            ("device", self.local_id.to_string()),
            ("token", token.to_string()),
        ];

        let response = self
            .client
            .get(url)
            .query(&params)
            .send()
            .await?
            .json::<AuthClientResponse>()
            .await?;

        Ok(response.data)
    }
}
