use futures::Future;
use hyper::service::{make_service_fn, service_fn};
use hyper::{server::Server, Body, Request, Response};
use prometheus::{register_int_counter, register_int_counter_vec, Encoder, IntCounter, IntCounterVec, TextEncoder};
use tracing::{error, info};

lazy_static! {
    pub static ref SIDECAR_SHUTDOWNS: IntCounterVec = register_int_counter_vec!(
        "sidecar_shutdowns",
        "Number of sidecar shutdowns",
        &["container", "job_name", "namespace"],
    )
    .unwrap();
    pub static ref FAILED_SIDECAR_SHUTDOWNS: IntCounterVec = register_int_counter_vec!(
        "failed_sidecar_shutdowns",
        "Number of failed sidecar shutdowns",
        &["container", "job_name", "namespace"],
    )
    .unwrap();
    pub static ref TOTAL_SIDECARS_SHUTDOWN: IntCounter =
        register_int_counter!("total_sidecar_shutdowns", "Total number of sidecars shut down",).unwrap();
}

/// The function which triggers on any request to the server (incl. any path)
async fn metric_service(_req: Request<Body>) -> hyper::Result<Response<Body>> {
    let encoder = TextEncoder::new();
    let mut buffer = vec![];
    let mf = prometheus::gather();
    encoder.encode(&mf, &mut buffer).unwrap();
    Ok(Response::builder()
        .header(hyper::header::CONTENT_TYPE, encoder.format_type())
        .body(Body::from(buffer))
        .unwrap())
}

/// The function which spawns the prometheus server
///
/// F is generally a Notify awaiting a notification
pub async fn prometheus_server<F>(port: u16, shutdown: F) -> hyper::Result<()>
where
    F: Future<Output = ()>,
{
    let addr = ([127, 0, 0, 1], port).into();
    info!("serving prometheus on http://{addr}");

    let service = make_service_fn(move |_| async { Ok::<_, hyper::Error>(service_fn(metric_service)) });
    let err = Server::bind(&addr)
        .serve(service)
        .with_graceful_shutdown(shutdown)
        .await;
    match &err {
        Ok(()) => info!("stopped prometheus server successfully"),
        Err(e) => error!("error while shutting down: {e}"),
    }
    Ok(())
}

#[tokio::test]
async fn prometheus_server_shuts_down_gracefully() {
    use hyper::{body::HttpBody, Client};
    use std::sync::Arc;
    use tokio::sync::Notify;

    let port = 1337;
    let shutdown = Arc::new(Notify::new());
    let shutdown_clone = shutdown.clone();
    let server = tokio::spawn(async move {
        prometheus_server(port, shutdown_clone.notified()).await.unwrap();
    });

    // do some prometheus stuff while we're at it
    SIDECAR_SHUTDOWNS.with_label_values(&["abc", "def", "ghi"]).inc();
    SIDECAR_SHUTDOWNS.with_label_values(&["abc", "def", "ghi"]).inc();

    let client = Client::new();
    let mut res = client
        .get(format!("http://localhost:{port}/").parse().unwrap())
        .await
        .unwrap();
    let mut buffer = String::new();
    while let Some(chunk) = res.body_mut().data().await {
        buffer += &String::from_utf8_lossy(&chunk.unwrap().to_vec());
    }
    assert!(buffer.contains("sidecar_shutdowns{container=\"abc\",job_name=\"def\",namespace=\"ghi\"} 2"));

    shutdown.notify_one();
    let ret = server.await;
    assert!(ret.is_ok())
}
