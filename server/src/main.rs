use protocol::{ClientMsg, Direction, PlayerState, ServerMsg, recv_msg, send_msg};
use tokio::io::{ReadHalf, WriteHalf};
use tokio::net::{TcpListener, TcpStream};

const WIDTH: u16 = 80;
const HEIGHT: u16 = 24;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let listener = TcpListener::bind("127.0.0.1:9000").await?;
    println!("[server] Listening on 127.0.0.1:9000");

    // Wait for player 0
    let (stream0, addr0) = listener.accept().await?;
    println!("[server] Player 0 connected from {addr0}");
    let (mut r0, mut w0) = tokio::io::split(stream0);
    println!("[server] Sending WaitingForOpponent to player 0");
    send_msg(&mut w0, &ServerMsg::WaitingForOpponent).await?;

    // Wait for player 1
    let (stream1, addr1) = listener.accept().await?;
    println!("[server] Player 1 connected from {addr1}");
    let (mut r1, mut w1) = tokio::io::split(stream1);

    // Send Welcome to both
    println!("[server] Sending Welcome to player 0");
    send_msg(
        &mut w0,
        &ServerMsg::Welcome {
            player_id: 0,
            width: WIDTH,
            height: HEIGHT,
        },
    )
    .await?;
    println!("[server] Sending Welcome to player 1");
    send_msg(
        &mut w1,
        &ServerMsg::Welcome {
            player_id: 1,
            width: WIDTH,
            height: HEIGHT,
        },
    )
    .await?;

    // Initialize player positions
    let mut players = [
        PlayerState {
            x: WIDTH / 3,
            y: HEIGHT / 2,
            connected: true,
        },
        PlayerState {
            x: WIDTH * 2 / 3,
            y: HEIGHT / 2,
            connected: true,
        },
    ];

    // Send initial game state
    println!("[server] Sending initial GameState to both players");
    broadcast(&mut w0, &mut w1, &players).await?;

    // Main game loop
    println!("[server] Entering game loop");
    game_loop(&mut r0, &mut r1, &mut w0, &mut w1, &mut players).await
}

async fn broadcast(
    w0: &mut WriteHalf<TcpStream>,
    w1: &mut WriteHalf<TcpStream>,
    players: &[PlayerState; 2],
) -> std::io::Result<()> {
    let msg = ServerMsg::GameState {
        players: players.clone(),
    };
    send_msg(w0, &msg).await?;
    send_msg(w1, &msg).await?;
    Ok(())
}

fn apply_move(player: &mut PlayerState, dir: Direction) {
    match dir {
        Direction::Up => player.y = player.y.saturating_sub(1),
        Direction::Down => {
            if player.y < HEIGHT - 1 {
                player.y += 1;
            }
        }
        Direction::Left => player.x = player.x.saturating_sub(1),
        Direction::Right => {
            if player.x < WIDTH - 1 {
                player.x += 1;
            }
        }
    }
}

async fn game_loop(
    r0: &mut ReadHalf<TcpStream>,
    r1: &mut ReadHalf<TcpStream>,
    w0: &mut WriteHalf<TcpStream>,
    w1: &mut WriteHalf<TcpStream>,
    players: &mut [PlayerState; 2],
) -> std::io::Result<()> {
    loop {
        tokio::select! {
            result = recv_msg::<_, ClientMsg>(r0) => {
                match result {
                    Ok(ClientMsg::Move(dir)) => {
                        println!("[server] Player 0 moved {dir:?}");
                        apply_move(&mut players[0], dir);
                        broadcast(w0, w1, players).await?;
                    }
                    Err(e) => {
                        println!("[server] Player 0 disconnected: {e}");
                        let _ = send_msg(w1, &ServerMsg::OpponentDisconnected).await;
                        break;
                    }
                }
            }
            result = recv_msg::<_, ClientMsg>(r1) => {
                match result {
                    Ok(ClientMsg::Move(dir)) => {
                        println!("[server] Player 1 moved {dir:?}");
                        apply_move(&mut players[1], dir);
                        broadcast(w0, w1, players).await?;
                    }
                    Err(e) => {
                        println!("[server] Player 1 disconnected: {e}");
                        let _ = send_msg(w0, &ServerMsg::OpponentDisconnected).await;
                        break;
                    }
                }
            }
        }
    }
    println!("[server] Game ended");
    Ok(())
}
