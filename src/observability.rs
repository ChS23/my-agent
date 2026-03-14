use anyhow::Result;
use opentelemetry::KeyValue;
use opentelemetry_otlp::{SpanExporter, WithExportConfig, WithHttpConfig};
use opentelemetry_sdk::trace::SdkTracerProvider;

/// Initialize OpenTelemetry with OTLP exporter pointed at Langfuse.
///
/// Supports two configuration modes:
/// 1. Standard OTEL env vars: OTEL_EXPORTER_OTLP_ENDPOINT + OTEL_EXPORTER_OTLP_HEADERS
/// 2. Langfuse env vars: LANGFUSE_PUBLIC_KEY + LANGFUSE_SECRET_KEY + LANGFUSE_HOST
///
/// Returns None if neither is configured (disabled).
pub fn init_langfuse() -> Result<Option<SdkTracerProvider>> {
    let (endpoint, headers) = match std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT") {
        Ok(ep) if !ep.is_empty() => {
            let headers = parse_otel_headers();
            tracing::info!(endpoint = %ep, "langfuse: using OTEL env vars");
            (ep.trim_end_matches('/').to_string(), headers)
        }
        _ => {
            let public_key = match std::env::var("LANGFUSE_PUBLIC_KEY") {
                Ok(k) if !k.is_empty() => k,
                _ => {
                    tracing::info!("langfuse: disabled (no keys configured)");
                    return Ok(None);
                }
            };
            let secret_key = std::env::var("LANGFUSE_SECRET_KEY")
                .map_err(|_| anyhow::anyhow!("LANGFUSE_SECRET_KEY required when LANGFUSE_PUBLIC_KEY is set"))?;
            let host = std::env::var("LANGFUSE_HOST")
                .or_else(|_| std::env::var("LANGFUSE_BASE_URL"))
                .unwrap_or_else(|_| "https://cloud.langfuse.com".to_string());
            let host = host.trim_end_matches('/').to_string();

            use base64::Engine;
            let auth = base64::engine::general_purpose::STANDARD
                .encode(format!("{public_key}:{secret_key}"));

            let endpoint = format!("{host}/api/public/otel/v1/traces");
            let headers = std::collections::HashMap::from([(
                "Authorization".to_string(),
                format!("Basic {auth}"),
            )]);

            tracing::info!(%endpoint, "langfuse: using LANGFUSE keys");
            (endpoint, headers)
        }
    };

    let exporter = SpanExporter::builder()
        .with_http()
        .with_endpoint(&endpoint)
        .with_headers(headers)
        .build()?;

    let provider = SdkTracerProvider::builder()
        .with_resource(
            opentelemetry_sdk::Resource::builder()
                .with_attributes([KeyValue::new("service.name", "my-agent")])
                .build(),
        )
        .with_batch_exporter(exporter)
        .build();

    opentelemetry::global::set_tracer_provider(provider.clone());

    tracing::info!(%endpoint, "langfuse: enabled");
    Ok(Some(provider))
}

/// Parse OTEL_EXPORTER_OTLP_HEADERS env var ("key=value,key2=value2" format).
fn parse_otel_headers() -> std::collections::HashMap<String, String> {
    let mut headers = std::collections::HashMap::new();
    if let Ok(raw) = std::env::var("OTEL_EXPORTER_OTLP_HEADERS") {
        for pair in raw.split(',') {
            if let Some((k, v)) = pair.split_once('=') {
                headers.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
    }
    headers
}
