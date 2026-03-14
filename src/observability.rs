use anyhow::Result;
use opentelemetry::KeyValue;
use opentelemetry_otlp::{SpanExporter, WithExportConfig, WithHttpConfig};
use opentelemetry_sdk::trace::SdkTracerProvider;

/// Initialize OpenTelemetry with OTLP exporter pointed at Langfuse.
/// Returns None if LANGFUSE_PUBLIC_KEY is not set (disabled).
pub fn init_langfuse() -> Result<Option<SdkTracerProvider>> {
    let public_key = match std::env::var("LANGFUSE_PUBLIC_KEY") {
        Ok(k) => k,
        Err(_) => return Ok(None),
    };
    let secret_key = std::env::var("LANGFUSE_SECRET_KEY")
        .map_err(|_| anyhow::anyhow!("LANGFUSE_SECRET_KEY required when LANGFUSE_PUBLIC_KEY is set"))?;
    let host = std::env::var("LANGFUSE_HOST")
        .unwrap_or_else(|_| "https://cloud.langfuse.com".to_string());

    // Langfuse expects Basic auth: base64(public_key:secret_key)
    use base64::Engine;
    let auth = base64::engine::general_purpose::STANDARD.encode(format!("{public_key}:{secret_key}"));

    let endpoint = format!("{host}/api/public/otel/v1/traces");

    let exporter = SpanExporter::builder()
        .with_http()
        .with_endpoint(&endpoint)
        .with_headers(std::collections::HashMap::from([(
            "Authorization".to_string(),
            format!("Basic {auth}"),
        )]))
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

    tracing::info!(host = %host, "Langfuse enabled via OTLP");
    Ok(Some(provider))
}
