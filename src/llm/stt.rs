use anyhow::Result;
use frankenstein::reqwest;

pub struct SttClient {
    client: reqwest::Client,
    api_base: String,
    api_key: String,
    model: String,
}

impl SttClient {
    pub fn new(api_key: &str, api_base: &str, model: &str) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_base: api_base.to_string(),
            api_key: api_key.to_string(),
            model: model.to_string(),
        }
    }

    /// Transcribe audio bytes (OGG/Opus from Telegram voice messages).
    pub async fn transcribe(&self, audio_data: Vec<u8>, filename: &str) -> Result<String> {
        let part = reqwest::multipart::Part::bytes(audio_data)
            .file_name(filename.to_string())
            .mime_str("audio/ogg")?;

        let form = reqwest::multipart::Form::new()
            .text("model", self.model.clone())
            .text("response_format", "text")
            .part("file", part);

        let url = format!("{}/audio/transcriptions", self.api_base);

        let response = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .multipart(form)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("STT failed ({status}): {body}");
        }

        let text = response.text().await?.trim().to_string();
        Ok(text)
    }
}
