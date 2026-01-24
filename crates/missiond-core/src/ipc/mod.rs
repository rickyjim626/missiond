//! Cross-platform IPC module
//!
//! Provides unified IPC abstractions:
//! - Unix: Uses Unix domain sockets
//! - Windows: Uses TCP loopback (127.0.0.1)
//!
//! The TCP approach on Windows is simpler and more reliable than named pipes.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

// ========== Platform-specific path handling ==========

/// Get the IPC endpoint identifier
/// - Unix: Returns socket file path
/// - Windows: Returns port number as string
pub fn ipc_endpoint(base_dir: &Path, name: &str) -> String {
    #[cfg(unix)]
    {
        base_dir.join(format!("{}.sock", name)).to_string_lossy().to_string()
    }

    #[cfg(windows)]
    {
        // Use a deterministic port based on name hash to avoid conflicts
        let _ = base_dir;
        let port = ipc_port_for_name(name);
        format!("127.0.0.1:{}", port)
    }
}

/// Get default mission home directory
pub fn default_mission_home() -> PathBuf {
    if let Ok(home) = std::env::var("XJP_MISSION_HOME") {
        return PathBuf::from(home);
    }
    dirs::home_dir()
        .map(|h| h.join(".xjp-mission"))
        .unwrap_or_else(|| PathBuf::from(".xjp-mission"))
}

/// Get the default IPC endpoint for missiond
pub fn default_ipc_endpoint() -> String {
    ipc_endpoint(&default_mission_home(), "missiond")
}

#[cfg(windows)]
fn ipc_port_for_name(name: &str) -> u16 {
    // Use a simple hash to get a port in the range 49152-65535 (dynamic ports)
    let hash: u32 = name.bytes().fold(0u32, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u32));
    let port = 49152 + (hash % 16383) as u16;
    port
}

// ========== Unix Implementation ==========

#[cfg(unix)]
mod platform {
    use super::*;
    use tokio::net::{UnixListener, UnixStream};
    use std::pin::Pin;
    use std::task::{Context as TaskContext, Poll};

    pub struct IpcListener {
        inner: UnixListener,
        path: PathBuf,
    }

    impl IpcListener {
        pub async fn bind(endpoint: &str) -> Result<Self> {
            let path = PathBuf::from(endpoint);

            // Remove stale socket if exists
            if path.exists() {
                if UnixStream::connect(&path).await.is_ok() {
                    anyhow::bail!("Another instance is already listening on {}", path.display());
                }
                std::fs::remove_file(&path).ok();
            }

            // Create parent directory
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).ok();
            }

            let inner = UnixListener::bind(&path)
                .with_context(|| format!("Failed to bind IPC socket: {}", path.display()))?;

            Ok(Self { inner, path })
        }

        pub async fn accept(&self) -> Result<IpcStream> {
            let (stream, _) = self.inner.accept().await?;
            Ok(IpcStream { inner: stream })
        }

        pub fn endpoint(&self) -> String {
            self.path.to_string_lossy().to_string()
        }
    }

    pub struct IpcStream {
        inner: UnixStream,
    }

    impl IpcStream {
        pub async fn connect(endpoint: &str) -> Result<Self> {
            let path = PathBuf::from(endpoint);
            let inner = UnixStream::connect(&path)
                .await
                .with_context(|| format!("Failed to connect to {}", path.display()))?;
            Ok(Self { inner })
        }

        pub async fn can_connect(endpoint: &str) -> bool {
            let path = PathBuf::from(endpoint);
            UnixStream::connect(&path).await.is_ok()
        }
    }

    impl AsyncRead for IpcStream {
        fn poll_read(
            mut self: Pin<&mut Self>,
            cx: &mut TaskContext<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            Pin::new(&mut self.inner).poll_read(cx, buf)
        }
    }

    impl AsyncWrite for IpcStream {
        fn poll_write(
            mut self: Pin<&mut Self>,
            cx: &mut TaskContext<'_>,
            buf: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            Pin::new(&mut self.inner).poll_write(cx, buf)
        }

        fn poll_flush(
            mut self: Pin<&mut Self>,
            cx: &mut TaskContext<'_>,
        ) -> Poll<std::io::Result<()>> {
            Pin::new(&mut self.inner).poll_flush(cx)
        }

        fn poll_shutdown(
            mut self: Pin<&mut Self>,
            cx: &mut TaskContext<'_>,
        ) -> Poll<std::io::Result<()>> {
            Pin::new(&mut self.inner).poll_shutdown(cx)
        }
    }
}

// ========== Windows Implementation (TCP) ==========

#[cfg(windows)]
mod platform {
    use super::*;
    use tokio::net::{TcpListener, TcpStream};
    use std::pin::Pin;
    use std::task::{Context as TaskContext, Poll};

    pub struct IpcListener {
        inner: TcpListener,
        endpoint: String,
    }

    impl IpcListener {
        pub async fn bind(endpoint: &str) -> Result<Self> {
            // Check if already listening
            if TcpStream::connect(endpoint).await.is_ok() {
                anyhow::bail!("Another instance is already listening on {}", endpoint);
            }

            let inner = TcpListener::bind(endpoint)
                .await
                .with_context(|| format!("Failed to bind TCP listener: {}", endpoint))?;

            Ok(Self {
                inner,
                endpoint: endpoint.to_string(),
            })
        }

        pub async fn accept(&self) -> Result<IpcStream> {
            let (stream, _) = self.inner.accept().await?;
            Ok(IpcStream { inner: stream })
        }

        pub fn endpoint(&self) -> String {
            self.endpoint.clone()
        }
    }

    pub struct IpcStream {
        inner: TcpStream,
    }

    impl IpcStream {
        pub async fn connect(endpoint: &str) -> Result<Self> {
            let inner = TcpStream::connect(endpoint)
                .await
                .with_context(|| format!("Failed to connect to {}", endpoint))?;
            Ok(Self { inner })
        }

        pub async fn can_connect(endpoint: &str) -> bool {
            TcpStream::connect(endpoint).await.is_ok()
        }
    }

    impl AsyncRead for IpcStream {
        fn poll_read(
            mut self: Pin<&mut Self>,
            cx: &mut TaskContext<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            Pin::new(&mut self.inner).poll_read(cx, buf)
        }
    }

    impl AsyncWrite for IpcStream {
        fn poll_write(
            mut self: Pin<&mut Self>,
            cx: &mut TaskContext<'_>,
            buf: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            Pin::new(&mut self.inner).poll_write(cx, buf)
        }

        fn poll_flush(
            mut self: Pin<&mut Self>,
            cx: &mut TaskContext<'_>,
        ) -> Poll<std::io::Result<()>> {
            Pin::new(&mut self.inner).poll_flush(cx)
        }

        fn poll_shutdown(
            mut self: Pin<&mut Self>,
            cx: &mut TaskContext<'_>,
        ) -> Poll<std::io::Result<()>> {
            Pin::new(&mut self.inner).poll_shutdown(cx)
        }
    }
}

// Re-export platform types
pub use platform::{IpcListener, IpcStream};

// ========== Common Utilities ==========

/// Send a newline-delimited message over IPC
pub async fn send_message<W: AsyncWrite + Unpin>(writer: &mut W, message: &str) -> Result<()> {
    writer.write_all(message.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;
    Ok(())
}

/// Receive a newline-delimited message from IPC
pub async fn recv_message<R: AsyncRead + Unpin>(reader: &mut BufReader<R>) -> Result<String> {
    let mut line = String::new();
    let bytes = reader.read_line(&mut line).await?;
    if bytes == 0 {
        anyhow::bail!("Connection closed");
    }
    Ok(line.trim().to_string())
}

/// Create a buffered reader from an IpcStream
pub fn buffered(stream: IpcStream) -> BufReader<IpcStream> {
    BufReader::new(stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_mission_home() {
        let home = default_mission_home();
        // Should end with .xjp-mission
        assert!(home.to_string_lossy().contains("xjp-mission"));
    }

    #[cfg(unix)]
    #[test]
    fn test_ipc_endpoint_unix() {
        let home = PathBuf::from("/home/user/.xjp-mission");
        let endpoint = ipc_endpoint(&home, "missiond");
        assert_eq!(endpoint, "/home/user/.xjp-mission/missiond.sock");
    }

    #[cfg(windows)]
    #[test]
    fn test_ipc_endpoint_windows() {
        let home = PathBuf::from(r"C:\Users\user\.xjp-mission");
        let endpoint = ipc_endpoint(&home, "missiond");
        assert!(endpoint.starts_with("127.0.0.1:"));
    }
}
