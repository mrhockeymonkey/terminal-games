use rand::Rng;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

// --- Grid constants ---

pub const GRID_COLS: usize = 15;
pub const GRID_ROWS: usize = 13;
/// The playable interior (inside the border walls).
pub const INNER_COLS: usize = GRID_COLS - 2; // 13
pub const INNER_ROWS: usize = GRID_ROWS - 2; // 11

pub const MAX_PLAYERS: usize = 5;

// --- Tile types ---

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TileType {
    Floor,
    BorderWall,
    HardBlock,
    SoftBlock,
}

// --- Map ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Map {
    /// Row-major grid: tiles[row][col].
    pub tiles: [[TileType; GRID_COLS]; GRID_ROWS],
}

impl Map {
    /// Returns the tile at (col, row), or None if out of bounds.
    pub fn get(&self, col: usize, row: usize) -> Option<TileType> {
        if col < GRID_COLS && row < GRID_ROWS {
            Some(self.tiles[row][col])
        } else {
            None
        }
    }

    /// Sets the tile at (col, row). Panics if out of bounds.
    pub fn set(&mut self, col: usize, row: usize, tile: TileType) {
        self.tiles[row][col] = tile;
    }

    /// Returns true if the tile at (col, row) blocks movement.
    pub fn is_blocked(&self, col: usize, row: usize) -> bool {
        match self.get(col, row) {
            Some(TileType::Floor) => false,
            _ => true, // walls, soft blocks, or out of bounds
        }
    }

    /// Generate a new arena map for the given number of players.
    ///
    /// Returns the map and a Vec of power-ups hidden under soft blocks.
    pub fn generate(num_players: usize) -> (Map, Vec<PowerUp>) {
        let mut rng = rand::rng();
        let mut tiles = [[TileType::Floor; GRID_COLS]; GRID_ROWS];

        // 1. Border walls on the perimeter.
        for col in 0..GRID_COLS {
            tiles[0][col] = TileType::BorderWall;
            tiles[GRID_ROWS - 1][col] = TileType::BorderWall;
        }
        for row in 0..GRID_ROWS {
            tiles[row][0] = TileType::BorderWall;
            tiles[row][GRID_COLS - 1] = TileType::BorderWall;
        }

        // 2. Hard blocks in lattice pattern inside the border.
        //    Inner area coordinates: col 1..=13, row 1..=11 (0-indexed in grid).
        //    Hard block where both inner_col and inner_row are even (0-indexed within inner area).
        //    inner_col = col - 1, inner_row = row - 1.
        //    So: hard block at grid positions where (col - 1) % 2 == 1 && (row - 1) % 2 == 1
        //    i.e. col % 2 == 0 && row % 2 == 0.
        for row in 1..GRID_ROWS - 1 {
            for col in 1..GRID_COLS - 1 {
                if col % 2 == 0 && row % 2 == 0 {
                    tiles[row][col] = TileType::HardBlock;
                }
            }
        }

        // 3. Collect spawn-protected tiles.
        let num = num_players.min(MAX_PLAYERS);
        let mut protected = std::collections::HashSet::new();
        for i in 0..num {
            for (c, r) in spawn_clear_tiles(i) {
                protected.insert((c, r));
            }
        }

        // 4. Place soft blocks on ~65% of remaining floor tiles.
        let mut floor_positions = Vec::new();
        for row in 1..GRID_ROWS - 1 {
            for col in 1..GRID_COLS - 1 {
                if tiles[row][col] == TileType::Floor && !protected.contains(&(col, row)) {
                    floor_positions.push((col, row));
                }
            }
        }

        let soft_block_count = (floor_positions.len() as f64 * 0.65) as usize;
        // Shuffle and take the first soft_block_count positions.
        shuffle(&mut rng, &mut floor_positions);
        let soft_positions = &floor_positions[..soft_block_count];

        for &(col, row) in soft_positions {
            tiles[row][col] = TileType::SoftBlock;
        }

        let mut map = Map { tiles };

        // 5. Distribute power-ups hidden under soft blocks.
        let power_ups = distribute_power_ups(&mut rng, &map);

        // Ensure spawn tiles are floor (safety check).
        for i in 0..num {
            for (c, r) in spawn_clear_tiles(i) {
                map.set(c, r, TileType::Floor);
            }
        }

        (map, power_ups)
    }
}

// --- Map generation helpers ---

/// Fisher-Yates shuffle.
fn shuffle<R: Rng + ?Sized, T>(rng: &mut R, slice: &mut [T]) {
    for i in (1..slice.len()).rev() {
        let j = rng.random_range(0..=i);
        slice.swap(i, j);
    }
}

/// Distribute power-ups under soft blocks.
///
/// Roughly: 6 BombUp, 6 FireUp, 4 SpeedUp, 1 FullFire per map.
fn distribute_power_ups<R: Rng + ?Sized>(rng: &mut R, map: &Map) -> Vec<PowerUp> {
    let mut soft_positions: Vec<(usize, usize)> = Vec::new();
    for row in 1..GRID_ROWS - 1 {
        for col in 1..GRID_COLS - 1 {
            if map.tiles[row][col] == TileType::SoftBlock {
                soft_positions.push((col, row));
            }
        }
    }
    shuffle(rng, &mut soft_positions);

    let distribution = [
        (PowerUpType::BombUp, 6),
        (PowerUpType::FireUp, 6),
        (PowerUpType::SpeedUp, 4),
        (PowerUpType::FullFire, 1),
    ];

    let mut power_ups = Vec::new();
    let mut idx = 0;
    for (kind, count) in distribution {
        for _ in 0..count {
            if idx >= soft_positions.len() {
                break;
            }
            let (col, row) = soft_positions[idx];
            power_ups.push(PowerUp {
                col,
                row,
                kind,
                revealed: false,
            });
            idx += 1;
        }
    }

    power_ups
}

// --- Direction ---

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}

// --- Player ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerState {
    /// Grid column (0-based).
    pub col: usize,
    /// Grid row (0-based).
    pub row: usize,
    pub alive: bool,
    pub connected: bool,
    /// Maximum number of bombs this player can have active simultaneously.
    pub bomb_max: u8,
    /// Number of bombs currently placed on the field by this player.
    pub bombs_active: u8,
    /// Blast radius in each direction (number of tiles beyond the bomb tile).
    pub fire_range: u8,
    /// Speed level (0 = base speed, each increment makes the player faster).
    pub speed: u8,
    /// Number of round wins accumulated in the current match.
    pub wins: u8,
}

impl PlayerState {
    pub fn new(col: usize, row: usize) -> Self {
        Self {
            col,
            row,
            alive: true,
            connected: true,
            bomb_max: 1,
            bombs_active: 0,
            fire_range: 1,
            speed: 0,
            wins: 0,
        }
    }

    /// Reset stats for a new round (keeps wins and connected status).
    pub fn reset_for_round(&mut self, col: usize, row: usize) {
        self.col = col;
        self.row = row;
        self.alive = true;
        self.bomb_max = 1;
        self.bombs_active = 0;
        self.fire_range = 1;
        self.speed = 0;
    }
}

// --- Bombs ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BombState {
    pub col: usize,
    pub row: usize,
    /// Index of the player who placed this bomb.
    pub owner: u8,
    /// Ticks remaining before detonation.
    pub fuse_ticks: u16,
    /// Blast radius at time of placement (snapshot of player's fire_range).
    pub fire_range: u8,
}

// --- Explosions ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExplosionTile {
    pub col: usize,
    pub row: usize,
    /// Ticks remaining before this explosion tile disappears.
    pub ticks_remaining: u16,
}

// --- Power-ups ---

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PowerUpType {
    BombUp,
    FireUp,
    SpeedUp,
    FullFire,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PowerUp {
    pub col: usize,
    pub row: usize,
    pub kind: PowerUpType,
    /// Whether the power-up is visible (soft block above it has been destroyed).
    pub revealed: bool,
}

// --- Game phase ---

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GamePhase {
    Lobby,
    Playing,
    RoundOver,
    MatchOver,
}

// --- Full game state ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameState {
    pub map: Map,
    pub players: Vec<PlayerState>,
    pub bombs: Vec<BombState>,
    pub explosions: Vec<ExplosionTile>,
    pub power_ups: Vec<PowerUp>,
    pub round: u8,
    /// Ticks remaining in the current round (before sudden death).
    pub round_ticks_remaining: u32,
    pub phase: GamePhase,
}

// --- Player starting positions ---

/// Returns the starting (col, row) for a given player index (0-4).
/// Positions are in the corners and center-top for player 5.
pub fn spawn_position(player_index: usize) -> (usize, usize) {
    match player_index {
        0 => (1, 1),                          // top-left
        1 => (GRID_COLS - 2, 1),              // top-right
        2 => (1, GRID_ROWS - 2),              // bottom-left
        3 => (GRID_COLS - 2, GRID_ROWS - 2),  // bottom-right
        4 => (GRID_COLS / 2, 1),              // center-top
        _ => (1, 1),                           // fallback
    }
}

/// Returns the tiles that must be cleared around a spawn position
/// (the spawn tile plus 2 adjacent tiles to give the player room).
pub fn spawn_clear_tiles(player_index: usize) -> Vec<(usize, usize)> {
    let (c, r) = spawn_position(player_index);
    let mut tiles = vec![(c, r)];
    match player_index {
        0 => {
            // top-left corner: clear right and down
            tiles.push((c + 1, r));
            tiles.push((c, r + 1));
        }
        1 => {
            // top-right corner: clear left and down
            tiles.push((c - 1, r));
            tiles.push((c, r + 1));
        }
        2 => {
            // bottom-left corner: clear right and up
            tiles.push((c + 1, r));
            tiles.push((c, r - 1));
        }
        3 => {
            // bottom-right corner: clear left and up
            tiles.push((c - 1, r));
            tiles.push((c, r - 1));
        }
        4 => {
            // center-top: clear left and right
            tiles.push((c - 1, r));
            tiles.push((c + 1, r));
        }
        _ => {}
    }
    tiles
}

// --- Client messages ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClientMsg {
    Move(Direction),
    PlaceBomb,
}

// --- Server messages ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServerMsg {
    Welcome {
        player_id: u8,
    },
    WaitingForPlayers,
    /// Full game state broadcast (sent every tick during play).
    State(GameState),
    RoundStart {
        round: u8,
    },
    RoundOver {
        /// Index of the winning player, or None if draw.
        winner: Option<u8>,
    },
    MatchOver {
        /// Index of the match winner.
        winner: u8,
    },
    PlayerDisconnected {
        player_id: u8,
    },
}

// --- Wire helpers (unchanged) ---

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
