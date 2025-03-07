a simple crate that fetches data from different sources

supports tar, http, folders(as search paths), std::fs


Has a "file_server" feature, to serve files inside DataSource using axum:

```rust
use data_source::DataSource;
use data_source::file_server::*;
let mut data_source = DataSource::FileMap(Default::default());
let app = axum::Router::new();
let app = register_data_source_route(app,  "/files/*path", data_source);
let service = DataSourceService::new(data_source);
let app = app.route_service(
    "/files/*path",
    get(|path: AxumPath<String>, service: DataSourceService| async {
        handle_file_request(path, &mut service).await
    })
```

