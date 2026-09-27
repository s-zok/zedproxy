mod proxy;
mod rewrite;
mod upstream;

fn main() {
    // tracing with default stdout/stderr fmt output; RUST_LOG controls the level.
    // The tracing-log bridge routes Pingora's internal `log` records into the same
    // subscriber so everything appears in one stream.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "zedproxy=info".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    let _ = tracing_log::LogTracer::init();

    let upstream_url = std::env::var("ZEDPROXY_UPSTREAM_URL")
        .unwrap_or_else(|_| "https://api.isoquant.ai/v1".to_string());

    let reasoning_effort =
        std::env::var("ZEDPROXY_REASONING_EFFORT").unwrap_or_else(|_| "high".to_string());

    let upstream = match upstream::Upstream::from_env(&upstream_url) {
        Ok(u) => u,
        Err(e) => {
            eprintln!("zedproxy: {e}");
            std::process::exit(1);
        }
    };

    let mut server = pingora::server::Server::new(None).unwrap();
    server.bootstrap();

    let mut service = pingora::proxy::http_proxy_service(
        &server.configuration,
        proxy::Proxy::new(upstream, reasoning_effort),
    );
    service.add_tcp("127.0.0.1:4000"); // approved: loopback only
    server.add_service(service);

    server.run_forever();
}
