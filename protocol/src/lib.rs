use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClientMsg {
    Move(Direction),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerState {
    pub x: u16,
    pub y: u16,
    pub connected: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServerMsg {
    Welcome {
        player_id: u8,
        width: u16,
        height: u16,
    },
    WaitingForOpponent,
    GameState {
        players: [PlayerState; 2],
    },
    OpponentDisconnected,
}

pub async fn send_msg<W: AsyncWriteExt + Unpin, M: Serialize>(
    writer: &mut W,
    msg: &M,
) -> std::io::Result<()> {
    let payload = serde_json::to_vec(msg)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let len = payload.len() as u32;
    writer.write_all(&len.to_be_bytes()).await?;
    writer.write_all(&payload).await?;
    writer.flush().await?;
    Ok(())
}

pub async fn recv_msg<R: AsyncReadExt + Unpin, M: DeserializeOwned>(
    reader: &mut R,
) -> std::io::Result<M> {
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;
    let mut payload = vec![0u8; len];
    reader.read_exact(&mut payload).await?;
    serde_json::from_slice(&payload)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}
