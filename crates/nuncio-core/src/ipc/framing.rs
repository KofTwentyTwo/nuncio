//! Length-prefixed binary frame codec for Nuncio IPC protocol.

use std::io::Error as IoError;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Maximum allowed payload size per frame (16 MB).
pub const MAX_FRAME_SIZE: usize = 16 * 1024 * 1024;

/// Read a 4-byte big-endian length-prefixed payload frame from an async reader.
pub async fn read_frame<R>(reader: &mut R) -> Result<Vec<u8>, IoError>
where
    R: AsyncRead + Unpin,
{
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf).await?;
    let length = u32::from_be_bytes(len_buf) as usize;

    if length > MAX_FRAME_SIZE {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Frame payload size {length} exceeds maximum limit of {MAX_FRAME_SIZE} bytes"),
        ));
    }

    let mut payload = vec![0u8; length];
    reader.read_exact(&mut payload).await?;
    Ok(payload)
}

/// Write a 4-byte big-endian length-prefixed payload frame to an async writer.
pub async fn write_frame<W>(writer: &mut W, payload: &[u8]) -> Result<(), IoError>
where
    W: AsyncWrite + Unpin,
{
    if payload.len() > MAX_FRAME_SIZE {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "Payload size {} exceeds maximum limit of {} bytes",
                payload.len(),
                MAX_FRAME_SIZE
            ),
        ));
    }

    let len_bytes = (payload.len() as u32).to_be_bytes();
    writer.write_all(&len_bytes).await?;
    writer.write_all(payload).await?;
    writer.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;

    #[tokio::test]
    async fn frame_codec_roundtrip_success() {
        let (mut client, mut server) = duplex(1024);
        let test_payload = b"{\"jsonrpc\":\"2.0\",\"method\":\"ping\"}";

        let write_task = tokio::spawn(async move {
            write_frame(&mut client, test_payload).await.unwrap();
        });

        let read_payload = read_frame(&mut server).await.unwrap();
        write_task.await.unwrap();

        assert_eq!(&read_payload[..], test_payload);
    }

    #[tokio::test]
    async fn frame_codec_oversized_payload_fails() {
        let (mut client, _server) = duplex(1024);
        let oversized = vec![0u8; MAX_FRAME_SIZE + 1];

        let result = write_frame(&mut client, &oversized).await;
        assert!(result.is_err());
    }
}
