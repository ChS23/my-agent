use anyhow::Result;
use chrono::{DateTime, Utc};
use tokio_rusqlite::Connection;
use tokio_rusqlite::rusqlite;

const AUTH_URL: &str = "https://ticktick.com/oauth/authorize";
const TOKEN_URL: &str = "https://ticktick.com/oauth/token";
const REDIRECT_URI: &str = "http://localhost:8080/callback";

#[derive(Debug, Clone)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Clone)]
pub struct TokenStore {
    conn: Connection,
    client_id: String,
    client_secret: String,
}

impl TokenStore {
    pub async fn new(db_path: &str, client_id: String, client_secret: String) -> Result<Self> {
        if let Some(parent) = std::path::Path::new(db_path).parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(db_path).await?;

        conn.call(|db| {
            db.execute_batch(
                "CREATE TABLE IF NOT EXISTS oauth_tokens (
                     service TEXT PRIMARY KEY,
                     access_token TEXT NOT NULL,
                     refresh_token TEXT NOT NULL,
                     expires_at TEXT NOT NULL
                 );",
            )?;
            Ok::<_, rusqlite::Error>(())
        })
        .await?;

        Ok(Self {
            conn,
            client_id,
            client_secret,
        })
    }

    /// Get the authorization URL for the user to visit.
    pub fn auth_url(&self) -> String {
        format!(
            "{}?client_id={}&redirect_uri={}&response_type=code&scope=tasks:read%20tasks:write&state=auth",
            AUTH_URL, self.client_id, REDIRECT_URI
        )
    }

    /// Exchange authorization code for tokens.
    pub async fn exchange_code(&self, code: &str) -> Result<Tokens> {
        let client = frankenstein::reqwest::Client::new();

        let resp = client
            .post(TOKEN_URL)
            .basic_auth(&self.client_id, Some(&self.client_secret))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(format!(
                "grant_type=authorization_code&code={}&redirect_uri={}",
                code, REDIRECT_URI
            ))
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("token exchange failed: {status} — {body}");
        }

        let json: serde_json::Value = serde_json::from_str(&resp.text().await?)?;
        let access_token = json["access_token"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("no access_token in response"))?
            .to_string();
        let refresh_token = json["refresh_token"]
            .as_str()
            .unwrap_or("")
            .to_string();
        let expires_in = json["expires_in"].as_i64().unwrap_or(3600);
        let expires_at = Utc::now() + chrono::Duration::seconds(expires_in);

        let tokens = Tokens {
            access_token,
            refresh_token,
            expires_at,
        };

        self.save_tokens(&tokens).await?;
        tracing::info!("TickTick tokens saved");

        Ok(tokens)
    }

    /// Refresh the access token using the refresh token.
    async fn refresh_tokens(&self, refresh_token: &str) -> Result<Tokens> {
        let client = frankenstein::reqwest::Client::new();

        let resp = client
            .post(TOKEN_URL)
            .basic_auth(&self.client_id, Some(&self.client_secret))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(format!(
                "grant_type=refresh_token&refresh_token={}",
                refresh_token
            ))
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("token refresh failed: {status} — {body}");
        }

        let json: serde_json::Value = serde_json::from_str(&resp.text().await?)?;
        let access_token = json["access_token"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("no access_token in refresh response"))?
            .to_string();
        let new_refresh = json["refresh_token"]
            .as_str()
            .unwrap_or(refresh_token)
            .to_string();
        let expires_in = json["expires_in"].as_i64().unwrap_or(3600);
        let expires_at = Utc::now() + chrono::Duration::seconds(expires_in);

        let tokens = Tokens {
            access_token,
            refresh_token: new_refresh,
            expires_at,
        };

        self.save_tokens(&tokens).await?;
        tracing::debug!("TickTick tokens refreshed");

        Ok(tokens)
    }

    /// Get a valid access token, refreshing if needed.
    pub async fn get_access_token(&self) -> Result<String> {
        let tokens = self.load_tokens().await?;

        match tokens {
            Some(t) => {
                // Refresh 5 min before expiry
                if Utc::now() + chrono::Duration::minutes(5) >= t.expires_at {
                    let refreshed = self.refresh_tokens(&t.refresh_token).await?;
                    Ok(refreshed.access_token)
                } else {
                    Ok(t.access_token)
                }
            }
            None => anyhow::bail!(
                "TickTick not authorized. Send /ticktick_auth to start OAuth flow."
            ),
        }
    }

    /// Check if we have valid tokens.
    pub async fn is_authorized(&self) -> bool {
        self.load_tokens().await.ok().flatten().is_some()
    }

    async fn save_tokens(&self, tokens: &Tokens) -> Result<()> {
        let at = tokens.access_token.clone();
        let rt = tokens.refresh_token.clone();
        let exp = tokens.expires_at.to_rfc3339();

        self.conn
            .call(move |db| {
                db.execute(
                    "INSERT INTO oauth_tokens (service, access_token, refresh_token, expires_at)
                     VALUES ('ticktick', ?1, ?2, ?3)
                     ON CONFLICT(service) DO UPDATE SET
                         access_token = excluded.access_token,
                         refresh_token = excluded.refresh_token,
                         expires_at = excluded.expires_at",
                    rusqlite::params![at, rt, exp],
                )?;
                Ok::<_, rusqlite::Error>(())
            })
            .await?;
        Ok(())
    }

    async fn load_tokens(&self) -> Result<Option<Tokens>> {
        let row = self
            .conn
            .call(|db| {
                let mut stmt = db.prepare(
                    "SELECT access_token, refresh_token, expires_at FROM oauth_tokens WHERE service = 'ticktick'",
                )?;
                let result = stmt.query_row([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                });
                match result {
                    Ok(r) => Ok::<_, rusqlite::Error>(Some(r)),
                    Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                    Err(e) => Err(e),
                }
            })
            .await?;

        match row {
            Some((at, rt, exp)) => {
                let expires_at: DateTime<Utc> = exp.parse()?;
                Ok(Some(Tokens {
                    access_token: at,
                    refresh_token: rt,
                    expires_at,
                }))
            }
            None => Ok(None),
        }
    }

    /// Start a local HTTP server to capture the OAuth callback.
    /// Returns the authorization code.
    pub async fn wait_for_callback() -> Result<String> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:8080").await?;
        tracing::info!("OAuth callback server listening on http://localhost:8080/callback");

        let (mut stream, _) = listener.accept().await?;

        let mut buf = vec![0u8; 4096];
        let n = stream.read(&mut buf).await?;
        let request = String::from_utf8_lossy(&buf[..n]);

        // Parse code from: GET /callback?code=XXX&state=auth HTTP/1.1
        let code = request
            .lines()
            .next()
            .and_then(|line| {
                let path = line.split_whitespace().nth(1)?;
                let query = path.split('?').nth(1)?;
                query
                    .split('&')
                    .find(|p| p.starts_with("code="))
                    .map(|p| p[5..].to_string())
            })
            .ok_or_else(|| anyhow::anyhow!("no code in callback"))?;

        // Send success response
        let response = "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n\
            <html><body><h1>Authorized!</h1><p>You can close this tab.</p></body></html>";
        stream.write_all(response.as_bytes()).await?;
        stream.flush().await?;

        Ok(code)
    }
}
