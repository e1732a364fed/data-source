use std::{collections::HashMap, io, path::Path, time::SystemTime};

use log::{debug, warn};

#[derive(thiserror::Error, Debug)]
pub enum FetchError {
    #[cfg(feature = "reqwest")]
    #[error("reqwest err")]
    R(#[from] reqwest::Error),
    #[error("io err")]
    I(#[from] io::Error),
    #[error("time err")]
    T(#[from] std::time::SystemTimeError),
    #[error("size limit exceed")]
    S,
    #[error("no cache file")]
    NC,
    #[error("not found")]
    NF,
    #[error("not found in directories `{0:?}`")]
    NFD(Vec<String>),
}

#[derive(Debug, Clone)]
pub struct FileCache {
    pub update_interval_seconds: Option<u64>,
    pub cache_file_path: Option<String>,
}

impl FileCache {
    pub fn read_cache_file(&self) -> Result<Vec<u8>, FetchError> {
        let cf = self.cache_file_path.as_ref().unwrap();
        let s: Vec<u8> = std::fs::read(cf)?;
        Ok(s)
    }

    #[cfg(feature = "tokio")]
    pub async fn read_cache_file_async(&self) -> Result<Vec<u8>, FetchError> {
        let cf = self.cache_file_path.as_ref().unwrap();

        let content = tokio::fs::read(cf).await?;
        Ok(content)
    }

    pub fn write_cache_file(&self, bytes: &[u8]) -> bool {
        let cf = self.cache_file_path.as_ref().unwrap();
        if let Err(err) = std::fs::write(cf, bytes) {
            warn!("Failed to write cache file: {err}");
            false
        } else {
            true
        }
    }

    #[cfg(feature = "tokio")]
    pub async fn write_cache_file_async(&self, bytes: &[u8]) -> bool {
        let cf = self.cache_file_path.as_ref().unwrap();
        if let Err(err) = tokio::fs::write(cf, bytes).await {
            warn!("Failed to write cache file: {err}");
            false
        } else {
            true
        }
    }

    /// 检查缓存文件是否超时
    pub fn is_cache_timeout(&self) -> Result<Option<bool>, FetchError> {
        if let Some(cf) = &self.cache_file_path {
            if std::fs::exists(cf)? {
                let mut expired = false;
                if let Some(interval) = self.update_interval_seconds {
                    let metadata = std::fs::metadata(cf)?;
                    let last_modified = metadata.modified()?;
                    let elapsed = SystemTime::now().duration_since(last_modified)?.as_secs();
                    expired = elapsed > interval;
                }
                return Ok(Some(expired));
            }
            Ok(None)
        } else {
            Ok(None)
        }
    }
}

#[cfg(feature = "tokio")]
#[async_trait::async_trait]
pub trait AsyncSource: Send + Sync {
    async fn fetch_async(&self) -> Result<Vec<u8>, FetchError>;
}

pub trait SyncSource {
    fn fetch(&self) -> Result<Vec<u8>, FetchError>;
}

#[cfg(feature = "tokio")]
pub async fn fetch_with_cache_async(
    fc: &FileCache,
    s: &dyn AsyncSource,
) -> Result<Vec<u8>, FetchError> {
    if fc.is_cache_timeout()?.is_some_and(|timeout| !timeout) {
        fc.read_cache_file_async().await
    } else {
        let d = s.fetch_async().await?;
        if fc.cache_file_path.is_some() {
            fc.write_cache_file_async(&d).await;
        }
        Ok(d)
    }
}
pub fn fetch_with_cache(fc: &FileCache, s: &dyn SyncSource) -> Result<Vec<u8>, FetchError> {
    if fc.is_cache_timeout()?.is_some_and(|timeout| !timeout) {
        fc.read_cache_file()
    } else {
        let d = s.fetch()?;
        if fc.cache_file_path.is_some() {
            fc.write_cache_file(&d);
        }
        Ok(d)
    }
}

#[cfg(feature = "tokio")]
#[async_trait::async_trait]
pub trait AsyncFolderSource: std::fmt::Debug {
    async fn get_file_content_async(
        &self,
        file_name: &std::path::Path,
    ) -> Result<(Vec<u8>, Option<String>), FetchError>;
}

pub trait SyncFolderSource: std::fmt::Debug {
    fn get_file_content(
        &self,
        file_name: &std::path::Path,
    ) -> Result<(Vec<u8>, Option<String>), FetchError>;
}

#[cfg(feature = "reqwest")]
#[derive(Clone, Debug, Default)]
pub struct HttpSource {
    pub url: String,
    pub proxy: Option<String>,
    pub custom_request_headers: Option<Vec<(String, String)>>,
    pub should_use_proxy: bool,
    pub size_limit_bytes: Option<usize>,
}

#[cfg(feature = "reqwest")]
impl HttpSource {
    pub fn get(
        &self,
        c: reqwest::blocking::Client,
    ) -> reqwest::Result<reqwest::blocking::Response> {
        let mut rb = c.get(&self.url);
        if let Some(h) = &self.custom_request_headers {
            for h in h.iter() {
                rb = rb.header(&h.0, &h.1);
            }
        }
        rb.send()
    }
    pub fn set_proxy(
        &self,
        mut cb: reqwest::blocking::ClientBuilder,
    ) -> reqwest::Result<reqwest::blocking::ClientBuilder> {
        let ps = self.proxy.as_ref().unwrap();
        let proxy = reqwest::Proxy::https(ps)?;
        cb = cb.proxy(proxy);
        let proxy = reqwest::Proxy::http(ps)?;
        Ok(cb.proxy(proxy))
    }
}
#[cfg(feature = "reqwest")]
impl SyncSource for HttpSource {
    fn fetch(&self) -> Result<Vec<u8>, FetchError> {
        let mut cb = reqwest::blocking::ClientBuilder::new();
        if self.should_use_proxy {
            cb = self.set_proxy(cb)?;
        }
        let c = cb.build()?;
        let r = self.get(c);
        let r = match r {
            Ok(r) => r,
            Err(e) => {
                if !self.should_use_proxy && self.proxy.is_some() {
                    let mut cb = reqwest::blocking::ClientBuilder::new();
                    cb = self.set_proxy(cb)?;
                    let c = cb.build()?;
                    self.get(c)?
                } else {
                    return Err(FetchError::R(e));
                }
            }
        };
        if let Some(sl) = self.size_limit_bytes {
            if let Some(s) = r.content_length() {
                if s as usize > sl {
                    return Err(FetchError::S);
                }
            }
        }
        let b = r.bytes()?;
        let v = b.to_vec();

        Ok(v)
    }
}

#[cfg(feature = "tokio")]
#[cfg(feature = "reqwest")]
impl HttpSource {
    pub async fn get_async(&self, client: reqwest::Client) -> reqwest::Result<reqwest::Response> {
        let mut request = client.get(&self.url);
        if let Some(headers) = &self.custom_request_headers {
            for (key, value) in headers {
                request = request.header(key, value);
            }
        }
        request.send().await
    }

    pub fn set_proxy_async(
        &self,
        client_builder: reqwest::ClientBuilder,
    ) -> reqwest::Result<reqwest::ClientBuilder> {
        let proxy = self.proxy.as_ref().unwrap();
        let client_builder = client_builder.proxy(reqwest::Proxy::http(proxy)?);
        let client_builder = client_builder.proxy(reqwest::Proxy::https(proxy)?);
        Ok(client_builder)
    }
}

#[cfg(feature = "tokio")]
#[cfg(feature = "reqwest")]
#[async_trait::async_trait]
impl AsyncSource for HttpSource {
    async fn fetch_async(&self) -> Result<Vec<u8>, FetchError> {
        let client_builder = reqwest::ClientBuilder::new();
        let client_builder = if self.should_use_proxy {
            self.set_proxy_async(client_builder)?
        } else {
            client_builder
        };
        let client = client_builder.build()?;

        let r = self.get_async(client).await;
        let response = match r {
            Ok(r) => r,
            Err(e) => {
                if !self.should_use_proxy && self.proxy.is_some() {
                    let mut cb = reqwest::ClientBuilder::new();
                    cb = self.set_proxy_async(cb)?;
                    let c = cb.build()?;
                    self.get_async(c).await?
                } else {
                    return Err(FetchError::R(e));
                }
            }
        };
        if let Some(size_limit) = self.size_limit_bytes {
            if let Some(content_length) = response.content_length() {
                if content_length as usize > size_limit {
                    return Err(FetchError::S);
                }
            }
        }

        let bytes = response.bytes().await?.to_vec();

        Ok(bytes)
    }
}

pub trait GetPath {
    fn get_path(&self) -> Option<String> {
        None
    }
}

#[derive(Debug)]
pub enum SingleFileSource {
    #[cfg(feature = "reqwest")]
    Http(HttpSource, FileCache),
    FilePath(String),
    Inline(Vec<u8>),
}
impl Default for SingleFileSource {
    fn default() -> Self {
        Self::Inline(Vec::new())
    }
}

impl GetPath for SingleFileSource {
    fn get_path(&self) -> Option<String> {
        match self {
            #[cfg(feature = "reqwest")]
            SingleFileSource::Http(http_source, _fc) => Some(http_source.url.clone()),
            SingleFileSource::FilePath(p) => Some(p.clone()),
            SingleFileSource::Inline(_ec) => None,
        }
    }
}

#[cfg(feature = "tokio")]
#[async_trait::async_trait]
impl AsyncSource for SingleFileSource {
    async fn fetch_async(&self) -> Result<Vec<u8>, FetchError> {
        match self {
            #[cfg(feature = "reqwest")]
            SingleFileSource::Http(http_source, fc) => {
                fetch_with_cache_async(fc, http_source).await
            }
            SingleFileSource::FilePath(f) => {
                let s: Vec<u8> = tokio::fs::read(f).await?;
                Ok(s)
            }
            SingleFileSource::Inline(v) => Ok(v.clone()),
        }
    }
}

impl SyncSource for SingleFileSource {
    fn fetch(&self) -> Result<Vec<u8>, FetchError> {
        match self {
            #[cfg(feature = "reqwest")]
            SingleFileSource::Http(http_source, fc) => fetch_with_cache(fc, http_source),
            SingleFileSource::FilePath(f) => {
                let s: Vec<u8> = std::fs::read(f)?;
                Ok(s)
            }
            SingleFileSource::Inline(v) => Ok(v.clone()),
        }
    }
}

/// Defines where to get the content of the requested file name.
///
/// 很多配置中 都要再加载其他外部文件,
/// FileSource 限定了 查找文件的 路径 和 来源, 读取文件时只会限制在这个范围内,
/// 这样就增加了安全性
#[derive(Debug, Default)]
pub enum DataSource {
    #[default]
    StdReadFile,
    Folders(Vec<String>), //从指定的一组路径来寻找文件

    #[cfg(feature = "tar")]
    Tar(Vec<u8>), // 从一个 已放到内存中的 tar 中 寻找文件

    /// 与其它方式不同，FileMap 存储名称的映射表, 无需遍历目录
    FileMap(HashMap<String, SingleFileSource>),

    Sync(Box<dyn SyncFolderSource + Send + Sync>),
    #[cfg(feature = "tokio")]
    Async(Box<dyn AsyncFolderSource + Send + Sync>),
}

impl DataSource {
    pub fn insert_current_working_dir(&mut self) -> io::Result<()> {
        if let DataSource::Folders(ref mut v) = self {
            v.push(std::env::current_dir()?.to_string_lossy().to_string())
        }
        Ok(())
    }

    pub fn read_to_string<P>(&self, file_name: P) -> Result<String, FetchError>
    where
        P: AsRef<std::path::Path>,
    {
        let r = SyncFolderSource::get_file_content(self, file_name.as_ref())?;
        Ok(String::from_utf8_lossy(r.0.as_slice()).to_string())
    }
}
#[cfg(feature = "tokio")]
#[async_trait::async_trait]
impl AsyncFolderSource for DataSource {
    /// 返回读到的 数据。可能还会返回 成功找到的路径
    async fn get_file_content_async(
        &self,
        file_name: &Path,
    ) -> Result<(Vec<u8>, Option<String>), FetchError> {
        match self {
            DataSource::Async(source) => source.get_file_content_async(file_name).await,

            DataSource::Sync(source) => source.get_file_content(file_name),
            #[cfg(feature = "tar")]
            DataSource::Tar(tar_binary) => get_file_from_tar(file_name, tar_binary),

            DataSource::Folders(possible_addrs) => {
                for dir in possible_addrs {
                    let real_file_name = std::path::Path::new(dir).join(file_name);

                    if real_file_name.exists() {
                        return Ok(tokio::fs::read(&real_file_name)
                            .await
                            .map(|v| (v, Some(dir.to_owned())))?);
                    }
                }
                Err(FetchError::NFD(possible_addrs.clone()))
            }
            DataSource::StdReadFile => {
                let s: Vec<u8> = tokio::fs::read(file_name).await?;
                Ok((s, None))
            }

            DataSource::FileMap(map) => {
                let r = map.get(&file_name.to_string_lossy().to_string());

                match r {
                    Some(sf) => sf.fetch_async().await.map(|d| (d, sf.get_path())),
                    None => Err(FetchError::NF),
                }
            }
        }
    }
}

impl SyncFolderSource for DataSource {
    /// 返回读到的 数据。可能还会返回 成功找到的路径
    fn get_file_content(&self, file_name: &Path) -> Result<(Vec<u8>, Option<String>), FetchError> {
        match self {
            DataSource::Sync(source) => source.get_file_content(file_name),

            #[cfg(feature = "tokio")]
            DataSource::Async(source) => {
                tokio::runtime::Handle::current().block_on(source.get_file_content_async(file_name))
            }

            #[cfg(feature = "tar")]
            DataSource::Tar(tar_binary) => get_file_from_tar(file_name, tar_binary),

            DataSource::Folders(possible_addrs) => {
                for dir in possible_addrs {
                    let real_file_name = std::path::Path::new(dir).join(file_name);

                    if real_file_name.exists() {
                        return Ok(
                            std::fs::read(&real_file_name).map(|v| (v, Some(dir.to_owned())))?
                        );
                    }
                }
                Err(FetchError::NFD(possible_addrs.clone()))
            }
            DataSource::StdReadFile => {
                let s: Vec<u8> = std::fs::read(file_name)?;
                Ok((s, None))
            }

            DataSource::FileMap(map) => {
                let r = map.get(&file_name.to_string_lossy().to_string());

                match r {
                    Some(sf) => sf.fetch().map(|d| (d, sf.get_path())),
                    None => Err(FetchError::NF),
                }
            }
        }
    }
}

#[cfg(feature = "tar")]
pub fn get_file_from_tar<P>(
    file_name_in_tar: P,
    tar_binary: &Vec<u8>,
) -> Result<(Vec<u8>, Option<String>), FetchError>
where
    P: AsRef<std::path::Path>,
{
    let mut a = tar::Archive::new(std::io::Cursor::new(tar_binary));

    debug!(
        "finding {} from tar, tar whole size is {}",
        file_name_in_tar.as_ref().to_str().unwrap(),
        tar_binary.len()
    );

    let mut e = a
        .entries()
        .unwrap()
        .find(|a| {
            a.as_ref()
                .is_ok_and(|b| b.path().is_ok_and(|c| c == file_name_in_tar.as_ref()))
        })
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "get_file_from_tar: can't find the file, {}",
                    file_name_in_tar.as_ref().to_str().unwrap()
                ),
            )
        })??;

    debug!("found {}", file_name_in_tar.as_ref().to_str().unwrap());

    let mut result = vec![];
    use std::io::Read;
    e.read_to_end(&mut result)?;
    Ok((
        result,
        Some(e.path().unwrap().to_str().unwrap().to_string()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use std::io::Write;
    use tempfile::TempDir;

    #[cfg(feature = "reqwest")]
    use reqwest::blocking::Client;

    const URL: &str = "https://www.rust-lang.org";

    #[cfg(feature = "tokio")]
    #[cfg(feature = "reqwest")]
    #[tokio::test]
    async fn test_http_source_fetch_async() {
        let http_source = HttpSource {
            url: URL.to_string(),
            should_use_proxy: false,
            ..Default::default()
        };

        let result = http_source.fetch_async().await;
        assert!(result.is_ok());
        assert!(!result.unwrap().is_empty());
    }

    #[cfg(feature = "reqwest")]
    #[test]
    fn test_http_source_fetch() {
        let http_source = HttpSource {
            url: URL.to_string(),
            should_use_proxy: false,
            ..Default::default()
        };

        let client = Client::new();
        let result = http_source.get(client);
        assert!(result.is_ok());
    }

    #[test]
    fn test_data_source_read_from_folders() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.txt");

        fs::write(&file_path, "hello world").unwrap();

        let data_source = DataSource::Folders(vec![temp_dir.path().to_string_lossy().to_string()]);

        let content = data_source.read_to_string("test.txt").unwrap();
        assert_eq!(content, "hello world");
    }

    #[test]
    fn test_data_source_read_from_file_map() {
        let file_map = vec![(
            "config.json".to_string(),
            SingleFileSource::Inline(b"{\"key\": \"value\"}".to_vec()),
        )]
        .into_iter()
        .collect();

        let data_source = DataSource::FileMap(file_map);

        let content = data_source.read_to_string("config.json").unwrap();
        assert_eq!(content, "{\"key\": \"value\"}");
    }

    #[cfg(feature = "tar")]
    #[test]
    fn test_get_file_from_tar() {
        let temp_dir = TempDir::new().unwrap();
        let tar_path = temp_dir.path().join("test.tar");

        let mut tar_builder = tar::Builder::new(File::create(&tar_path).unwrap());

        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(file, "hello tar").unwrap();
        let file_path = file.path().to_owned();

        tar_builder
            .append_path_with_name(&file_path, "test.txt")
            .unwrap();
        tar_builder.finish().unwrap();

        let tar_data = fs::read(&tar_path).unwrap();
        let result = get_file_from_tar("test.txt", &tar_data);

        assert!(result.is_ok());
        let (content, path) = result.unwrap();
        assert_eq!(String::from_utf8_lossy(&content), "hello tar\n");
        assert_eq!(path.unwrap(), "test.txt");
    }
}
