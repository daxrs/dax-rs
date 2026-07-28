use std::{
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};

use axum::{
    body::Body,
    http::{Request, Response, StatusCode},
    response::IntoResponse,
};
use futures::future::BoxFuture;
use tokio::sync::Semaphore;
use tokio::time::timeout;
use tower::{Layer, Service};

#[derive(Clone)]
pub struct ConcurrencyLimitLayer {
    semaphore: Arc<Semaphore>,
    max_wait: Duration,
}

impl ConcurrencyLimitLayer {
    pub fn new(max_concurrency: usize, max_wait: Duration) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(max_concurrency)),
            max_wait,
        }
    }
}

impl<S> Layer<S> for ConcurrencyLimitLayer {
    type Service = ConcurrencyLimitService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        ConcurrencyLimitService {
            inner,
            semaphore: self.semaphore.clone(),
            max_wait: self.max_wait,
        }
    }
}

#[derive(Clone)]
pub struct ConcurrencyLimitService<S> {
    inner: S,
    semaphore: Arc<Semaphore>,
    max_wait: Duration,
}

impl<S, ReqBody> Service<Request<ReqBody>> for ConcurrencyLimitService<S>
where
    S: Service<Request<ReqBody>, Response = Response<Body>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    ReqBody: Send + 'static,
{
    type Response = Response<Body>;
    type Error = S::Error;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<ReqBody>) -> Self::Future {
        let mut inner = self.inner.clone();
        let semaphore = self.semaphore.clone();
        let max_wait = self.max_wait;

        Box::pin(async move {
            let permit = {
                if let Ok(p) = semaphore.clone().try_acquire_owned() {
                    p
                } else {
                    tracing::warn!(
                        max_wait_secs = max_wait.as_secs(),
                        "concurrency limit reached — request queued"
                    );
                    match timeout(max_wait, semaphore.clone().acquire_owned()).await {
                        Ok(Ok(p)) => p,
                        _ => {
                            tracing::warn!("concurrency limit: request rejected after timeout");
                            return Ok(StatusCode::TOO_MANY_REQUESTS.into_response());
                        }
                    }
                }
            };

            let response = inner.call(req).await;

            drop(permit);

            response
        })
    }
}
