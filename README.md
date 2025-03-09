a simple crate that fetches data from different sources

supports tar, http, folders(as search paths), std::fs


Has a "file_server" feature, to serve files inside DataSource using axum:

```rust
use data_source::DataSource;
use data_source::file_server::*;
let mut data_source = DataSource::FileMap(Default::default());
let app = axum::Router::new();
let app = register_data_source_route(app,  "/files/{*path}", data_source);

```

Or

```rust
use data_source::DataSource;
use data_source::file_server::*;
let mut data_source = DataSource::FileMap(Default::default());
let app = axum::Router::new();
let service = DataSourceService::new(data_source);
 use axum::extract::Path as AxumPath;


async fn handle_file_request_wrapper(
    path: AxumPath<String>,
    State(mut service): State<DataSourceService>,
) -> impl IntoResponse {
    handle_file_request(path, &mut service).await
}

app = app.route(
    "/files/{*path}",
    get(handle_file_request_wrapper).with_state(service),
);
```
