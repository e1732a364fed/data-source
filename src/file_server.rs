use crate::*;
use axum::{
    body::Bytes,
    http::{header, Method, Request, StatusCode},
    response::{IntoResponse, Response},
};
use http_body_util::{combinators::UnsyncBoxBody, BodyExt, Full};
use std::{convert::Infallible, path::Path, sync::Arc};
use tower::{Service, ServiceBuilder};

#[derive(Clone, Debug)]
pub struct DataSourceService {
    data_source: Arc<DataSource>,
    // 可添加更多配置项，例如默认 Content-Type
}

impl DataSourceService {
    pub fn new(data_source: DataSource) -> Self {
        Self {
            data_source: Arc::new(data_source),
        }
    }
}

impl<ReqBody> Service<Request<ReqBody>> for DataSourceService
where
    ReqBody: Send + 'static,
{
    type Response = Response<UnsyncBoxBody<Bytes, std::io::Error>>;
    type Error = Infallible;
    type Future = futures_util::future::BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(
        &mut self,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Request<ReqBody>) -> Self::Future {
        let data_source = self.data_source.clone();

        Box::pin(async move {
            // 只处理 GET/HEAD 请求
            if !matches!(req.method(), &Method::GET | &Method::HEAD) {
                let body =
                    UnsyncBoxBody::new(Full::new(Bytes::from("Method not allowed")).map_err(
                        |_| std::io::Error::new(std::io::ErrorKind::Other, "stream error"),
                    ));
                return Ok(Response::builder()
                    .status(StatusCode::METHOD_NOT_ALLOWED)
                    .body(body)
                    .unwrap());
            }

            let path = req.uri().path().trim_start_matches("/files/");
            let path = Path::new(path);

            let result = data_source.get_file_content_async(path).await;

            // 构建响应
            match result {
                Ok((content, _)) => {
                    let mime = mime_guess::from_path(path).first_or_octet_stream();
                    let body = UnsyncBoxBody::new(Full::new(Bytes::from(content)).map_err(|_| {
                        std::io::Error::new(std::io::ErrorKind::Other, "stream error")
                    }));
                    let response = Response::builder()
                        .header(header::CONTENT_TYPE, mime.to_string())
                        .body(body)
                        .unwrap();
                    Ok(response)
                }
                Err(e) => {
                    let status = match e {
                        FetchError::NF | FetchError::NFD(_) => StatusCode::NOT_FOUND,
                        FetchError::S => StatusCode::PAYLOAD_TOO_LARGE,
                        _ => StatusCode::INTERNAL_SERVER_ERROR,
                    };
                    let body = UnsyncBoxBody::new(
                        Full::new(Bytes::from(
                            status.to_string()
                                + "\n\n"
                                + &path.to_string_lossy().to_string()
                                + "\n\n"
                                + &e.to_string(),
                        ))
                        .map_err(|_| {
                            std::io::Error::new(std::io::ErrorKind::Other, "stream error")
                        }),
                    );
                    Ok(Response::builder().status(status).body(body).unwrap())
                }
            }
        })
    }
}

use axum::extract::Path as AxumPath;

pub async fn handle_file_request(
    AxumPath(path): AxumPath<String>,
    service: &mut DataSourceService,
) -> impl IntoResponse {
    let req = Request::builder()
        .uri(format!("/{}", path))
        .body(())
        .unwrap();
    service.call(req).await.unwrap()
}

pub fn register_data_source_route(
    app: axum::Router,
    path: &str,
    data_source: DataSource,
) -> axum::Router {
    let service = DataSourceService::new(data_source);
    app.route_service(
        path,
        ServiceBuilder::new().service_fn(move |req| {
            let mut service = service.clone();
            async move { service.call(req).await }
        }),
    )
}
