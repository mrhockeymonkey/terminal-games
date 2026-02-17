use std::io::{self, Write};

use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{Event, EventStream, KeyCode, KeyEvent},
    execute,
    terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
};
use futures::StreamExt;
use protocol::{ClientMsg, Direction, ServerMsg, recv_msg, send_msg};
use tokio::net::TcpStream;

#[tokio::main]
async fn main() -> io::Result<()> {
    let stream = TcpStream::connect("127.0.0.1:9000").await?;
    let (mut reader, mut writer) = tokio::io::split(stream);

    // Wait for server messages before entering raw mode
    eprintln!("[client] Connected to server, waiting for initial message...");
    let msg: ServerMsg = recv_msg(&mut reader).await?;
    eprintln!("[client] Received: {msg:?}");

    let (player_id, width, height) = match msg {
        ServerMsg::WaitingForOpponent => {
            println!("Waiting for opponent...");
            let welcome: ServerMsg = recv_msg(&mut reader).await?;
            eprintln!("[client] Received: {welcome:?}");
            match welcome {
                ServerMsg::Welcome { player_id, width, height } => (player_id, width, height),
                other => {
                    eprintln!("[client] Expected Welcome after WaitingForOpponent, got: {other:?}");
                    return Ok(());
                }
            }
        }
        ServerMsg::Welcome { player_id, width, height } => (player_id, width, height),
        other => {
            eprintln!("[client] Expected WaitingForOpponent or Welcome, got: {other:?}");
            return Ok(());
        }
    };
    eprintln!("[client] Joined as player {player_id}, board {width}x{height}");

    // Enter raw mode and alternate screen
    let mut stdout = io::stdout();
    terminal::enable_raw_mode()?;
    execute!(stdout, EnterAlternateScreen, Hide)?;

    let result = run_game_loop(&mut reader, &mut writer, &mut stdout, player_id, width, height).await;

    // Cleanup always runs
    execute!(stdout, Show, LeaveAlternateScreen)?;
    terminal::disable_raw_mode()?;

    if let Err(e) = result {
        eprintln!("Game error: {e}");
    }

    Ok(())
}

async fn run_game_loop(
    reader: &mut (impl tokio::io::AsyncReadExt + Unpin),
    writer: &mut (impl tokio::io::AsyncWriteExt + Unpin),
    stdout: &mut io::Stdout,
    player_id: u8,
    width: u16,
    height: u16,
) -> io::Result<()> {
    let mut event_stream = EventStream::new();

    // Labels for each player
    let labels = if player_id == 0 {
        ["X", "Y"]
    } else {
        ["Y", "X"]
    };

    loop {
        tokio::select! {
            maybe_event = event_stream.next() => {
                let Some(Ok(event)) = maybe_event else { break };
                if let Event::Key(KeyEvent { code, .. }) = event {
                    let dir = match code {
                        KeyCode::Up => Some(Direction::Up),
                        KeyCode::Down => Some(Direction::Down),
                        KeyCode::Left => Some(Direction::Left),
                        KeyCode::Right => Some(Direction::Right),
                        KeyCode::Char('q') | KeyCode::Esc => break,
                        _ => None,
                    };
                    if let Some(d) = dir {
                        send_msg(writer, &ClientMsg::Move(d)).await?;
                    }
                }
            }
            result = recv_msg::<_, ServerMsg>(reader) => {
                match result? {
                    ServerMsg::GameState { players } => {
                        execute!(stdout, Clear(ClearType::All))?;

                        // Draw status line at top
                        execute!(stdout, MoveTo(0, 0))?;
                        write!(stdout, "You are Player {} | Board: {}x{}", labels[0], width, height)?;

                        // Draw players
                        for (i, p) in players.iter().enumerate() {
                            if p.connected {
                                let label = if i == player_id as usize { labels[0] } else { labels[1] };
                                execute!(stdout, MoveTo(p.x, p.y + 1))?;
                                write!(stdout, "{label}")?;
                            }
                        }
                        stdout.flush()?;
                    }
                    ServerMsg::OpponentDisconnected => {
                        execute!(stdout, Clear(ClearType::All), MoveTo(0, 0))?;
                        write!(stdout, "Opponent disconnected. Press any key to exit.")?;
                        stdout.flush()?;
                        // Wait for one more key press
                        loop {
                            if let Some(Ok(Event::Key(_))) = event_stream.next().await {
                                break;
                            }
                        }
                        break;
                    }
                    _ => {}
                }
            }
        }
    }

    Ok(())
}
