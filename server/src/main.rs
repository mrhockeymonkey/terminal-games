use protocol::{
    BombState, ClientMsg, Direction, ExplosionTile, GamePhase, GameState, Map, PlayerState,
    PowerUpType, ServerMsg, TileType, GRID_COLS, GRID_ROWS, recv_msg, send_msg,
    spawn_position,
};
use tokio::io::{AsyncWriteExt, WriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::time::{self, Duration};

// --- Configuration constants ---

const NUM_PLAYERS: usize = 2;
const TICK_RATE: u64 = 20;
const TICK_DURATION: Duration = Duration::from_millis(1000 / TICK_RATE);

const BOMB_FUSE_TICKS: u16 = 50;
const EXPLOSION_DURATION_TICKS: u16 = 10;

const ROUND_TICKS: u32 = 3600; // 3 minutes at 20 tps
const ROUND_OVER_PAUSE_TICKS: u32 = 60; // 3 seconds
const TARGET_WINS: u8 = 2;

const SUDDEN_DEATH_INTERVAL: u32 = 20; // drop a wall ring every 20 ticks

// Speed: base cooldown is 4 ticks, each speed level reduces by 1, min 1
fn move_cooldown(speed: u8) -> u32 {
    let cd = 4u32.saturating_sub(speed as u32);
    cd.max(1)
}

// --- Player input event ---

enum PlayerEvent {
    Message(u8, ClientMsg),
    Disconnected(u8),
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let listener = TcpListener::bind("127.0.0.1:9000").await?;
    println!("[server] Listening on 127.0.0.1:9000, waiting for {NUM_PLAYERS} players");

    let (tx, rx) = mpsc::channel::<PlayerEvent>(256);
    let mut writers: Vec<WriteHalf<TcpStream>> = Vec::new();

    // Accept players
    for i in 0..NUM_PLAYERS {
        let (stream, addr) = listener.accept().await?;
        println!("[server] Player {i} connected from {addr}");
        let (mut reader, mut writer) = tokio::io::split(stream);

        // Send Welcome
        send_msg(&mut writer, &ServerMsg::Welcome { player_id: i as u8 }).await?;

        // If not all players yet, send WaitingForPlayers to all connected players
        if i < NUM_PLAYERS - 1 {
            let joined = (i + 1) as u8;
            let needed = NUM_PLAYERS as u8;
            send_msg(&mut writer, &ServerMsg::WaitingForPlayers { joined, needed }).await?;
            // Notify already-waiting players of the updated count
            for w in writers.iter_mut() {
                send_msg(w, &ServerMsg::WaitingForPlayers { joined, needed }).await?;
            }
        }

        writers.push(writer);

        // Spawn reader task
        let tx_clone = tx.clone();
        let player_id = i as u8;
        tokio::spawn(async move {
            loop {
                match recv_msg::<_, ClientMsg>(&mut reader).await {
                    Ok(msg) => {
                        if tx_clone
                            .send(PlayerEvent::Message(player_id, msg))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(_) => {
                        let _ = tx_clone.send(PlayerEvent::Disconnected(player_id)).await;
                        break;
                    }
                }
            }
        });
    }

    // Drop the sender so only reader tasks hold senders
    drop(tx);

    println!("[server] All players connected, starting match");
    run_match(&mut writers, rx).await
}

// --- Match loop ---

async fn run_match(
    writers: &mut Vec<WriteHalf<TcpStream>>,
    mut rx: mpsc::Receiver<PlayerEvent>,
) -> std::io::Result<()> {
    let num_players = writers.len();

    // Initialize win counters (persist across rounds)
    let mut wins = vec![0u8; num_players];

    let mut round: u8 = 1;

    loop {
        // Generate map and set up game state
        let (map, power_ups) = Map::generate(num_players);

        let players: Vec<PlayerState> = (0..num_players)
            .map(|i| {
                let (col, row) = spawn_position(i);
                let mut p = PlayerState::new(col, row);
                p.wins = wins[i];
                p
            })
            .collect();

        let mut state = GameState {
            map,
            players: players.clone(),
            bombs: Vec::new(),
            explosions: Vec::new(),
            power_ups,
            round,
            round_ticks_remaining: ROUND_TICKS,
            phase: GamePhase::Playing,
        };

        // Broadcast RoundStart
        broadcast(writers, &ServerMsg::RoundStart { round }).await;

        // Per-player movement cooldown tracking
        let mut move_cooldowns = vec![0u32; num_players];
        let mut desired_dirs: Vec<Option<Direction>> = vec![None; num_players];

        // Sudden death state
        let mut sudden_death_active = false;
        let mut sudden_death_tick_counter: u32 = 0;
        let mut sudden_death_ring: usize = 0;

        // Round-over pause counter
        let mut round_over_countdown: Option<u32> = None;

        let mut interval = time::interval(TICK_DURATION);

        // Tick loop for this round
        'round_loop: loop {
            interval.tick().await;

            // Drain all pending player messages (non-blocking)
            loop {
                match rx.try_recv() {
                    Ok(PlayerEvent::Message(pid, msg)) => {
                        let id = pid as usize;
                        if id < num_players && state.players[id].alive {
                            match msg {
                                ClientMsg::Move(dir) => {
                                    desired_dirs[id] = Some(dir);
                                }
                                ClientMsg::PlaceBomb => {
                                    try_place_bomb(&mut state, id);
                                }
                            }
                        }
                    }
                    Ok(PlayerEvent::Disconnected(pid)) => {
                        let id = pid as usize;
                        if id < num_players {
                            state.players[id].connected = false;
                            state.players[id].alive = false;
                            broadcast(
                                writers,
                                &ServerMsg::PlayerDisconnected {
                                    player_id: pid,
                                },
                            )
                            .await;
                        }
                    }
                    Err(_) => break,
                }
            }

            // If we're in round-over pause, just count down
            if let Some(ref mut countdown) = round_over_countdown {
                *countdown -= 1;
                if *countdown == 0 {
                    break 'round_loop;
                }
                broadcast(writers, &ServerMsg::State(state.clone())).await;
                continue;
            }

            // --- Simulation ---

            // 1. Movement
            for i in 0..num_players {
                if !state.players[i].alive {
                    continue;
                }
                if move_cooldowns[i] > 0 {
                    move_cooldowns[i] -= 1;
                }
                if let Some(dir) = desired_dirs[i].take() {
                    if move_cooldowns[i] == 0 {
                        try_move(&mut state, i, dir);
                        move_cooldowns[i] = move_cooldown(state.players[i].speed);
                    }
                }
            }

            // 2. Power-up collection
            collect_power_ups(&mut state);

            // 3. Bomb fuse countdown & detonation
            tick_bombs(&mut state);

            // 4. Explosion timer countdown
            state
                .explosions
                .iter_mut()
                .for_each(|e| e.ticks_remaining = e.ticks_remaining.saturating_sub(1));
            state.explosions.retain(|e| e.ticks_remaining > 0);

            // 5. Kill detection (players on explosion tiles)
            check_kills(&mut state);

            // 6. Round timer / sudden death
            if state.round_ticks_remaining > 0 {
                state.round_ticks_remaining -= 1;
            }
            if state.round_ticks_remaining == 0 && !sudden_death_active {
                sudden_death_active = true;
                sudden_death_tick_counter = 0;
                sudden_death_ring = 0;
            }
            if sudden_death_active {
                sudden_death_tick_counter += 1;
                if sudden_death_tick_counter >= SUDDEN_DEATH_INTERVAL {
                    sudden_death_tick_counter = 0;
                    drop_sudden_death_ring(&mut state, sudden_death_ring);
                    sudden_death_ring += 1;
                    // Kill players on newly placed walls
                    check_wall_kills(&mut state);
                }
            }

            // 7. Check round end
            let alive_count = state.players.iter().filter(|p| p.alive).count();
            if alive_count <= 1 {
                // Determine winner
                let winner = state
                    .players
                    .iter()
                    .enumerate()
                    .find(|(_, p)| p.alive)
                    .map(|(i, _)| i as u8);

                if let Some(w) = winner {
                    wins[w as usize] += 1;
                    state.players[w as usize].wins = wins[w as usize];
                }

                state.phase = GamePhase::RoundOver;
                broadcast(writers, &ServerMsg::RoundOver { winner }).await;

                // Check match end
                if let Some(w) = winner {
                    if wins[w as usize] >= TARGET_WINS {
                        state.phase = GamePhase::MatchOver;
                        broadcast(writers, &ServerMsg::MatchOver { winner: w }).await;
                        broadcast(writers, &ServerMsg::State(state.clone())).await;
                        println!("[server] Match over! Player {w} wins!");
                        return Ok(());
                    }
                }

                round_over_countdown = Some(ROUND_OVER_PAUSE_TICKS);
            }

            // Broadcast state
            broadcast(writers, &ServerMsg::State(state.clone())).await;
        }

        // Next round
        round += 1;
    }
}

// --- Movement ---

fn try_move(state: &mut GameState, player_idx: usize, dir: Direction) {
    let p = &state.players[player_idx];
    let (col, row) = (p.col, p.row);

    let (new_col, new_row) = match dir {
        Direction::Up => (col, row.wrapping_sub(1)),
        Direction::Down => (col, row + 1),
        Direction::Left => (col.wrapping_sub(1), row),
        Direction::Right => (col + 1, row),
    };

    // Bounds check
    if new_col >= GRID_COLS || new_row >= GRID_ROWS {
        return;
    }

    // Tile collision
    if state.map.is_blocked(new_col, new_row) {
        // Try corner rounding
        if let Some((nudge_col, nudge_row)) = corner_round(state, col, row, dir) {
            state.players[player_idx].col = nudge_col;
            state.players[player_idx].row = nudge_row;
        }
        return;
    }

    // Bomb collision
    if state
        .bombs
        .iter()
        .any(|b| b.col == new_col && b.row == new_row)
    {
        return;
    }

    state.players[player_idx].col = new_col;
    state.players[player_idx].row = new_row;
}

/// Corner rounding: if a player is trying to move into a blocked tile but is
/// one tile off from a corridor, nudge them into the corridor.
fn corner_round(
    state: &GameState,
    col: usize,
    row: usize,
    dir: Direction,
) -> Option<(usize, usize)> {
    match dir {
        Direction::Up | Direction::Down => {
            // Moving vertically but blocked — check if one column left or right has an opening
            let target_row = if dir == Direction::Up {
                row.wrapping_sub(1)
            } else {
                row + 1
            };
            if target_row >= GRID_ROWS {
                return None;
            }

            // Check left nudge
            if col > 0
                && !state.map.is_blocked(col - 1, row)
                && !state.map.is_blocked(col - 1, target_row)
                && !has_bomb_at(state, col - 1, row)
            {
                return Some((col - 1, row));
            }
            // Check right nudge
            if col + 1 < GRID_COLS
                && !state.map.is_blocked(col + 1, row)
                && !state.map.is_blocked(col + 1, target_row)
                && !has_bomb_at(state, col + 1, row)
            {
                return Some((col + 1, row));
            }
        }
        Direction::Left | Direction::Right => {
            let target_col = if dir == Direction::Left {
                col.wrapping_sub(1)
            } else {
                col + 1
            };
            if target_col >= GRID_COLS {
                return None;
            }

            // Check up nudge
            if row > 0
                && !state.map.is_blocked(col, row - 1)
                && !state.map.is_blocked(target_col, row - 1)
                && !has_bomb_at(state, col, row - 1)
            {
                return Some((col, row - 1));
            }
            // Check down nudge
            if row + 1 < GRID_ROWS
                && !state.map.is_blocked(col, row + 1)
                && !state.map.is_blocked(target_col, row + 1)
                && !has_bomb_at(state, col, row + 1)
            {
                return Some((col, row + 1));
            }
        }
    }
    None
}

fn has_bomb_at(state: &GameState, col: usize, row: usize) -> bool {
    state.bombs.iter().any(|b| b.col == col && b.row == row)
}

// --- Bombs ---

fn try_place_bomb(state: &mut GameState, player_idx: usize) {
    let p = &state.players[player_idx];
    if !p.alive || p.bombs_active >= p.bomb_max {
        return;
    }
    let (col, row) = (p.col, p.row);

    // Don't place if there's already a bomb here
    if has_bomb_at(state, col, row) {
        return;
    }

    let fire_range = p.fire_range;
    state.bombs.push(BombState {
        col,
        row,
        owner: player_idx as u8,
        fuse_ticks: BOMB_FUSE_TICKS,
        fire_range,
    });
    state.players[player_idx].bombs_active += 1;
}

fn tick_bombs(state: &mut GameState) {
    // Decrement fuses
    for bomb in state.bombs.iter_mut() {
        bomb.fuse_ticks = bomb.fuse_ticks.saturating_sub(1);
    }

    // Detonate bombs with fuse == 0 (chain reactions handled iteratively)
    loop {
        let detonate_idx = state.bombs.iter().position(|b| b.fuse_ticks == 0);
        match detonate_idx {
            Some(idx) => {
                let bomb = state.bombs.remove(idx);
                detonate_bomb(state, &bomb);
            }
            None => break,
        }
    }
}

fn detonate_bomb(state: &mut GameState, bomb: &BombState) {
    // Decrement owner's active bomb count
    let owner = bomb.owner as usize;
    if owner < state.players.len() {
        state.players[owner].bombs_active = state.players[owner].bombs_active.saturating_sub(1);
    }

    let range = bomb.fire_range as usize;

    // Center tile
    add_explosion(state, bomb.col, bomb.row);

    // Four directions
    let directions: [(isize, isize); 4] = [(0, -1), (0, 1), (-1, 0), (1, 0)];
    for (dc, dr) in directions {
        for dist in 1..=range {
            let nc = bomb.col as isize + dc * dist as isize;
            let nr = bomb.row as isize + dr * dist as isize;
            if nc < 0 || nr < 0 || nc >= GRID_COLS as isize || nr >= GRID_ROWS as isize {
                break;
            }
            let (nc, nr) = (nc as usize, nr as usize);
            match state.map.get(nc, nr) {
                Some(TileType::BorderWall) | Some(TileType::HardBlock) => {
                    // Stop, don't include this tile
                    break;
                }
                Some(TileType::SoftBlock) => {
                    // Destroy this block, add explosion, but don't continue past
                    state.map.set(nc, nr, TileType::Floor);
                    add_explosion(state, nc, nr);
                    // Reveal any power-up hidden here
                    for pu in state.power_ups.iter_mut() {
                        if pu.col == nc && pu.row == nr {
                            pu.revealed = true;
                        }
                    }
                    break;
                }
                _ => {
                    add_explosion(state, nc, nr);
                    // Destroy revealed power-ups hit by explosions
                    state
                        .power_ups
                        .retain(|pu| !(pu.col == nc && pu.row == nr && pu.revealed));
                    // Chain reaction: detonate any bomb at this tile
                    if let Some(chain_idx) =
                        state.bombs.iter().position(|b| b.col == nc && b.row == nr)
                    {
                        let chain_bomb = state.bombs.remove(chain_idx);
                        detonate_bomb(state, &chain_bomb);
                    }
                }
            }
        }
    }
}

fn add_explosion(state: &mut GameState, col: usize, row: usize) {
    // Don't add duplicate explosion on the same tile
    if state
        .explosions
        .iter()
        .any(|e| e.col == col && e.row == row)
    {
        return;
    }
    state.explosions.push(ExplosionTile {
        col,
        row,
        ticks_remaining: EXPLOSION_DURATION_TICKS,
    });
}

// --- Power-up collection ---

fn collect_power_ups(state: &mut GameState) {
    let mut collected = Vec::new();
    for (pu_idx, pu) in state.power_ups.iter().enumerate() {
        if !pu.revealed {
            continue;
        }
        for (pi, p) in state.players.iter().enumerate() {
            if p.alive && p.col == pu.col && p.row == pu.row {
                collected.push((pu_idx, pi, pu.kind));
                break;
            }
        }
    }

    // Apply in reverse index order so removal indices stay valid
    collected.sort_by(|a, b| b.0.cmp(&a.0));
    for (pu_idx, pi, kind) in collected {
        apply_power_up(&mut state.players[pi], kind);
        state.power_ups.remove(pu_idx);
    }
}

fn apply_power_up(player: &mut PlayerState, kind: PowerUpType) {
    match kind {
        PowerUpType::BombUp => player.bomb_max = (player.bomb_max + 1).min(8),
        PowerUpType::FireUp => player.fire_range = (player.fire_range + 1).min(8),
        PowerUpType::SpeedUp => player.speed = (player.speed + 1).min(4),
        PowerUpType::FullFire => player.fire_range = 8,
    }
}

// --- Kill detection ---

fn check_kills(state: &mut GameState) {
    for p in state.players.iter_mut() {
        if !p.alive {
            continue;
        }
        if state
            .explosions
            .iter()
            .any(|e| e.col == p.col && e.row == p.row)
        {
            p.alive = false;
        }
    }
}

// --- Sudden death ---

/// Drop border walls inward in a spiral pattern.
/// `ring` is 0-indexed: ring 0 is the row/col just inside the existing border, etc.
fn drop_sudden_death_ring(state: &mut GameState, ring: usize) {
    // The border is already walls at row 0, col 0, etc.
    // Ring 0 affects row 1, row GRID_ROWS-2, col 1, col GRID_COLS-2
    let top = 1 + ring;
    let bottom = GRID_ROWS - 2 - ring;
    let left = 1 + ring;
    let right = GRID_COLS - 2 - ring;

    if top > bottom || left > right {
        return; // No more rings to drop
    }

    // Top row
    for col in left..=right {
        crush_tile(state, col, top);
    }
    // Bottom row
    if bottom != top {
        for col in left..=right {
            crush_tile(state, col, bottom);
        }
    }
    // Left column (excluding corners already done)
    for row in (top + 1)..bottom {
        crush_tile(state, left, row);
    }
    // Right column
    if right != left {
        for row in (top + 1)..bottom {
            crush_tile(state, right, row);
        }
    }
}

fn crush_tile(state: &mut GameState, col: usize, row: usize) {
    state.map.set(col, row, TileType::BorderWall);
    // Remove any bombs on this tile
    state.bombs.retain(|b| !(b.col == col && b.row == row));
    // Remove any power-ups on this tile
    state.power_ups.retain(|p| !(p.col == col && p.row == row));
    // Remove any explosions on this tile
    state
        .explosions
        .retain(|e| !(e.col == col && e.row == row));
}

fn check_wall_kills(state: &mut GameState) {
    for p in state.players.iter_mut() {
        if !p.alive {
            continue;
        }
        if state.map.is_blocked(p.col, p.row) {
            p.alive = false;
        }
    }
}

// --- Broadcasting ---

async fn broadcast(writers: &mut Vec<WriteHalf<TcpStream>>, msg: &ServerMsg) {
    let payload =
        serde_json::to_vec(msg).expect("failed to serialize ServerMsg");
    let len = (payload.len() as u32).to_be_bytes();

    for writer in writers.iter_mut() {
        let _ = writer.write_all(&len).await;
        let _ = writer.write_all(&payload).await;
        let _ = writer.flush().await;
    }
}
