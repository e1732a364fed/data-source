use std::{collections::HashMap, io};

use log::debug;

#[derive(thiserror::Error, Debug)]
pub enum FetchError {
    #[cfg(feature = "reqwest")]
    #[error("reqwest err")]
    R(#[from] reqwest::Error),
    #[error("io err")]
    I(#[from] io::Error),
    #[error("size limit exceed")]
    S,
}

#[cfg(feature = "reqwest")]
#[derive(Clone, Debug, Default)]
pub struct HttpSource {
    pub url: String,
    pub proxy: Option<String>,
    pub custom_request_headers: Option<Vec<(String, String)>>,
    pub should_use_proxy: bool,
    pub size_limit_bytes: Option<usize>,
    pub update_interval_seconds: Option<u64>,
}
#[cfg(feature = "reqwest")]
impl HttpSource {
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

    pub fn fetch(&self) -> Result<Vec<u8>, FetchError> {
        let mut cb = reqwest::blocking::ClientBuilder::new();
        if self.should_use_proxy {
            cb = self.set_proxy(cb)?;
        }
        let c = cb.build()?;
        let r = c.get(&self.url).send()?;
        if let Some(sl) = self.size_limit_bytes {
            if let Some(s) = r.content_length() {
                if s as usize > sl {
                    return Err(FetchError::S);
                }
            }
        }
        let b = r.bytes()?;
        Ok(b.to_vec())
    }
}

#[derive(Clone, Debug)]
pub enum SingleFileSource {
    #[cfg(feature = "reqwest")]
    Http(HttpSource),
    FilePath(String),
    Inline(Vec<u8>),
}
impl Default for SingleFileSource {
    fn default() -> Self {
        Self::Inline(Vec::new())
    }
}
/// Defines where to get the content of the requested file name.
///
/// 很多配置中 都要再加载其他外部文件,
/// FileSource 限定了 查找文件的 路径 和 来源, 读取文件时只会限制在这个范围内,
/// 这样就增加了安全性
#[derive(Clone, Debug, Default)]
pub enum DataSource {
    #[default]
    StdReadFile,
    Folders(Vec<String>), //从指定的一组路径来寻找文件

    #[cfg(feature = "tar")]
    Tar(Vec<u8>), // 从一个 已放到内存中的 tar 中 寻找文件

    /// 与其它方式不同，FileMap 存储名称的映射表, 无需遍历目录
    FileMap(HashMap<String, SingleFileSource>),
}

impl DataSource {
    pub fn insert_current_working_dir(&mut self) -> io::Result<()> {
        if let DataSource::Folders(ref mut v) = self {
            v.push(std::env::current_dir()?.to_string_lossy().to_string())
        }
        Ok(())
    }

    pub fn read_to_string<P>(&self, file_name: P) -> io::Result<String>
    where
        P: AsRef<std::path::Path>,
    {
        let r = self.get_file_content(file_name)?;
        Ok(String::from_utf8_lossy(r.0.as_slice()).to_string())
    }

    /// 返回读到的 数据。可能还会返回 成功找到的路径
    pub fn get_file_content<P>(&self, file_name: P) -> io::Result<(Vec<u8>, Option<String>)>
    where
        P: AsRef<std::path::Path>,
    {
        match self {
            #[cfg(feature = "tar")]
            DataSource::Tar(tar_binary) => get_file_from_tar(file_name, tar_binary),

            DataSource::Folders(possible_addrs) => {
                for dir in possible_addrs {
                    let real_file_name = std::path::Path::new(dir).join(file_name.as_ref());

                    if real_file_name.exists() {
                        return std::fs::read(&real_file_name).map(|v| (v, Some(dir.to_owned())));
                    }
                }
                Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!(
                        "File not found in specified directories: {:?}",
                        file_name.as_ref()
                    ),
                ))
            }
            DataSource::StdReadFile => {
                let s = std::fs::read_to_string(file_name)?;
                Ok((s.into_bytes(), None))
            }

            DataSource::FileMap(map) => {
                let r = map.get(&file_name.as_ref().to_string_lossy().to_string());

                match r {
                    Some(sf) => match sf {
                        #[cfg(feature = "reqwest")]
                        SingleFileSource::Http(http_source) => {
                            let r = http_source.fetch();
                            match r {
                                Ok(v) => Ok((v, None)),
                                Err(e) => Err(io::Error::other(e)),
                            }
                        }
                        SingleFileSource::FilePath(f) => {
                            let s = std::fs::read_to_string(f)?;
                            Ok((s.into_bytes(), None))
                        }
                        SingleFileSource::Inline(v) => Ok((v.clone(), None)),
                    },
                    None => Err(io::Error::new(io::ErrorKind::NotFound, "")),
                }
            }
        }
    }
}

#[cfg(feature = "tar")]
pub fn get_file_from_tar<P>(
    file_name_in_tar: P,
    tar_binary: &Vec<u8>,
) -> io::Result<(Vec<u8>, Option<String>)>
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
