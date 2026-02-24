use std::io::{self, Write};

use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{Event, EventStream, KeyCode, KeyEvent},
    execute, queue,
    style::{Attribute, Color, ResetColor, SetAttribute, SetForegroundColor},
    terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
};
use futures::StreamExt;
use protocol::{
    ClientMsg, Direction, GamePhase, GameState, ServerMsg, TileType, GRID_COLS, GRID_ROWS,
    recv_msg, send_msg,
};
use tokio::net::TcpStream;

// Each tile is 2 characters wide for better aspect ratio
const TILE_WIDTH: usize = 2;

// Player display characters and colors
const PLAYER_CHARS: [&str; 5] = ["P1", "P2", "P3", "P4", "P5"];
const PLAYER_COLORS: [Color; 5] = [
    Color::Green,
    Color::Cyan,
    Color::Magenta,
    Color::Yellow,
    Color::Blue,
];

// HUD is drawn below the map
const MAP_OFFSET_Y: u16 = 1; // leave row 0 for top HUD

// --- Double-buffered cell ---

#[derive(Clone, Copy, PartialEq, Eq)]
struct Cell {
    chars: [u8; 4], // up to 4 UTF-8 bytes for the 2-char-wide content (ASCII subset)
    len: u8,
    fg: Color,
    bold: bool,
}

impl Cell {
    const BLANK: Cell = Cell {
        chars: [b' ', b' ', 0, 0],
        len: 2,
        fg: Color::Reset,
        bold: false,
    };

    fn from_str(s: &str, fg: Color, bold: bool) -> Cell {
        let bytes = s.as_bytes();
        let mut chars = [0u8; 4];
        let len = bytes.len().min(4);
        chars[..len].copy_from_slice(&bytes[..len]);
        Cell {
            chars,
            len: len as u8,
            fg,
            bold,
        }
    }

    fn as_str(&self) -> &str {
        std::str::from_utf8(&self.chars[..self.len as usize]).unwrap_or("??")
    }
}

// --- Frame buffer for the map grid ---

struct FrameBuffer {
    cells: [[Cell; GRID_COLS]; GRID_ROWS],
}

impl FrameBuffer {
    fn new() -> Self {
        FrameBuffer {
            cells: [[Cell::BLANK; GRID_COLS]; GRID_ROWS],
        }
    }

    fn clear(&mut self) {
        for row in self.cells.iter_mut() {
            for cell in row.iter_mut() {
                *cell = Cell::BLANK;
            }
        }
    }
}

// --- HUD line buffer ---

const HUD_MAX_WIDTH: usize = 120;

#[derive(Clone, PartialEq, Eq)]
struct HudLine {
    /// Segments of (text, color, bold)
    segments: Vec<(String, Color, bool)>,
}

impl HudLine {
    fn new() -> Self {
        HudLine {
            segments: Vec::new(),
        }
    }

    fn push(&mut self, text: &str, fg: Color, bold: bool) {
        self.segments.push((text.to_string(), fg, bold));
    }

}

// --- Renderer with double buffering ---

struct Renderer {
    current: FrameBuffer,
    next: FrameBuffer,
    prev_top_hud: HudLine,
    prev_bottom_huds: Vec<HudLine>,
    first_frame: bool,
}

impl Renderer {
    fn new() -> Self {
        Renderer {
            current: FrameBuffer::new(),
            next: FrameBuffer::new(),
            prev_top_hud: HudLine::new(),
            prev_bottom_huds: Vec::new(),
            first_frame: true,
        }
    }

    fn render(
        &mut self,
        stdout: &mut io::Stdout,
        state: &GameState,
        player_id: u8,
        status_msg: Option<&str>,
    ) -> io::Result<()> {
        // Build the next frame buffer from game state
        self.next.clear();
        self.build_map_cells(state);

        // Build HUD lines
        let top_hud = self.build_top_hud(state, player_id);
        let bottom_huds = self.build_bottom_huds(state, player_id, status_msg);

        // On first frame, clear screen fully
        if self.first_frame {
            queue!(stdout, Clear(ClearType::All))?;
            self.first_frame = false;
        }

        // Diff and write top HUD
        if top_hud != self.prev_top_hud {
            queue!(stdout, MoveTo(0, 0))?;
            // Clear the line first
            write!(stdout, "{:width$}", "", width = HUD_MAX_WIDTH)?;
            queue!(stdout, MoveTo(0, 0))?;
            write_hud_line(stdout, &top_hud)?;
            self.prev_top_hud = top_hud;
        }

        // Diff and write map cells
        for row in 0..GRID_ROWS {
            for col in 0..GRID_COLS {
                let next_cell = self.next.cells[row][col];
                if next_cell != self.current.cells[row][col] {
                    let x = (col * TILE_WIDTH) as u16;
                    let y = row as u16 + MAP_OFFSET_Y;
                    queue!(stdout, MoveTo(x, y))?;
                    if next_cell.bold {
                        queue!(
                            stdout,
                            SetForegroundColor(next_cell.fg),
                            SetAttribute(Attribute::Bold)
                        )?;
                    } else {
                        queue!(stdout, SetForegroundColor(next_cell.fg))?;
                    }
                    write!(stdout, "{}", next_cell.as_str())?;
                    if next_cell.bold {
                        queue!(stdout, SetAttribute(Attribute::Reset))?;
                    }
                }
            }
        }

        // Diff and write bottom HUD
        let bottom_y = GRID_ROWS as u16 + MAP_OFFSET_Y + 1;
        for (i, hud) in bottom_huds.iter().enumerate() {
            let prev = self.prev_bottom_huds.get(i);
            if prev != Some(hud) {
                let y = bottom_y + i as u16;
                queue!(stdout, MoveTo(0, y))?;
                write!(stdout, "{:width$}", "", width = HUD_MAX_WIDTH)?;
                queue!(stdout, MoveTo(0, y))?;
                write_hud_line(stdout, hud)?;
            }
        }
        // Clear any extra previous bottom HUD lines that no longer exist
        for i in bottom_huds.len()..self.prev_bottom_huds.len() {
            let y = bottom_y + i as u16;
            queue!(stdout, MoveTo(0, y))?;
            write!(stdout, "{:width$}", "", width = HUD_MAX_WIDTH)?;
        }
        self.prev_bottom_huds = bottom_huds;

        queue!(stdout, ResetColor)?;
        stdout.flush()?;

        // Swap buffers
        std::mem::swap(&mut self.current, &mut self.next);

        Ok(())
    }

    fn build_map_cells(&mut self, state: &GameState) {
        // Base tiles
        for row in 0..GRID_ROWS {
            for col in 0..GRID_COLS {
                let cell = match state.map.tiles[row][col] {
                    TileType::Floor => Cell::BLANK,
                    TileType::BorderWall => Cell::from_str("##", Color::DarkGrey, false),
                    TileType::HardBlock => Cell::from_str("@@", Color::Grey, false),
                    TileType::SoftBlock => Cell::from_str("%%", Color::DarkYellow, false),
                };
                self.next.cells[row][col] = cell;
            }
        }

        // Revealed power-ups
        for pu in &state.power_ups {
            if !pu.revealed {
                continue;
            }
            let (ch, color) = match pu.kind {
                protocol::PowerUpType::BombUp => ("B+", Color::DarkYellow),
                protocol::PowerUpType::FireUp => ("F+", Color::Red),
                protocol::PowerUpType::SpeedUp => ("S+", Color::Cyan),
                protocol::PowerUpType::FullFire => ("FF", Color::Magenta),
            };
            self.next.cells[pu.row][pu.col] = Cell::from_str(ch, color, false);
        }

        // Bombs
        for b in &state.bombs {
            self.next.cells[b.row][b.col] = Cell::from_str("()", Color::DarkRed, false);
        }

        // Players (alive)
        for (i, p) in state.players.iter().enumerate() {
            if p.alive {
                let color = PLAYER_COLORS[i % PLAYER_COLORS.len()];
                let ch = PLAYER_CHARS[i % PLAYER_CHARS.len()];
                self.next.cells[p.row][p.col] = Cell::from_str(ch, color, true);
            }
        }

        // Dead players (ghosts)
        for p in &state.players {
            if !p.alive && p.connected {
                self.next.cells[p.row][p.col] = Cell::from_str("XX", Color::DarkGrey, false);
            }
        }

        // Explosions (highest priority, drawn last to overwrite)
        for exp in &state.explosions {
            let has_h = state.explosions.iter().any(|e| {
                e.row == exp.row && (e.col == exp.col + 1 || (exp.col > 0 && e.col == exp.col - 1))
            });
            let has_v = state.explosions.iter().any(|e| {
                e.col == exp.col && (e.row == exp.row + 1 || (exp.row > 0 && e.row == exp.row - 1))
            });
            let ch = if has_h && has_v {
                "++"
            } else if has_v {
                "||"
            } else if has_h {
                "=="
            } else {
                "++"
            };
            self.next.cells[exp.row][exp.col] = Cell::from_str(ch, Color::Red, true);
        }
    }

    fn build_top_hud(&self, state: &GameState, player_id: u8) -> HudLine {
        let mut hud = HudLine::new();
        let me = &state.players[player_id as usize];
        hud.push(
            &format!(
                "You: P{} | Bombs:{}/{} Fire:{} Speed:{} | Round {} | ",
                player_id + 1,
                me.bomb_max - me.bombs_active,
                me.bomb_max,
                me.fire_range,
                me.speed,
                state.round,
            ),
            Color::White,
            false,
        );

        let secs = state.round_ticks_remaining / 20;
        let mins = secs / 60;
        let secs = secs % 60;
        hud.push(&format!("Time: {mins}:{secs:02} | "), Color::White, false);

        for (i, p) in state.players.iter().enumerate() {
            hud.push(
                &format!("P{}:{} ", i + 1, p.wins),
                PLAYER_COLORS[i],
                false,
            );
        }

        hud
    }

    fn build_bottom_huds(
        &self,
        state: &GameState,
        player_id: u8,
        status_msg: Option<&str>,
    ) -> Vec<HudLine> {
        let mut lines = Vec::new();

        // Line 0: phase + death status
        let mut line0 = HudLine::new();
        let phase_str = match state.phase {
            GamePhase::Lobby => "LOBBY",
            GamePhase::Playing => "PLAYING",
            GamePhase::RoundOver => "ROUND OVER",
            GamePhase::MatchOver => "MATCH OVER",
        };
        line0.push(&format!("[{phase_str}]"), Color::White, false);

        if !state.players[player_id as usize].alive && state.phase == GamePhase::Playing {
            line0.push(" YOU ARE DEAD", Color::Red, false);
        }
        lines.push(line0);

        // Line 1: status message
        if let Some(msg) = status_msg {
            let mut line1 = HudLine::new();
            line1.push(msg, Color::Yellow, true);
            lines.push(line1);
        } else {
            lines.push(HudLine::new());
        }

        // Line 2: controls help
        let mut line2 = HudLine::new();
        line2.push("Arrows: move | Space: bomb | Q: quit", Color::DarkGrey, false);
        lines.push(line2);

        lines
    }
}

fn write_hud_line(stdout: &mut io::Stdout, hud: &HudLine) -> io::Result<()> {
    for (text, color, bold) in &hud.segments {
        queue!(stdout, SetForegroundColor(*color))?;
        if *bold {
            queue!(stdout, SetAttribute(Attribute::Bold))?;
        }
        write!(stdout, "{text}")?;
        if *bold {
            queue!(stdout, SetAttribute(Attribute::Reset))?;
        }
    }
    Ok(())
}

fn display_lobby_screen(stdout: &mut io::Stdout, player_id: u8, joined: u8, needed: u8) -> io::Result<()> {
    let logo = [
        r":::::::::   ::::::::  ::::    ::::  :::::::::   :::::::: ",
        r":+:    :+: :+:    :+: +:+:+: :+:+:+ :+:    :+: :+:    :+:",
        r"+:+    +:+ +:+    +:+ +:+ +:+:+ +:+ +:+    +:+ +:+       ",
        r"+#++:++#+  +#+    +:+ +#+  +:+  +#+ +#++:++#+  +#++:++#++",
        r"+#+    +#+ +#+    +#+ +#+       +#+ +#+    +#+        +#+",
        r"#+#    #+# #+#    #+# #+#       #+# #+#    #+# #+#    #+#",
        r"#########   ########  ###       ### #########   ######## ",
    ];

    let (term_cols, term_rows) = terminal::size().unwrap_or((80, 24));
    let logo_width = logo.iter().map(|l| l.len()).max().unwrap_or(0) as u16;
    let logo_height = logo.len() as u16;

    // Center vertically, leaving room for the status lines below
    let start_row = term_rows.saturating_sub(logo_height + 4) / 2;

    execute!(stdout, Clear(ClearType::All))?;

    // Draw logo with gradient: bright yellow (top) to deep red (bottom)
    let gradient: [Color; 7] = [
        Color::Rgb { r: 255, g: 255, b: 0 },   // bright yellow
        Color::Rgb { r: 255, g: 200, b: 0 },   // golden yellow
        Color::Rgb { r: 255, g: 150, b: 0 },   // orange
        Color::Rgb { r: 230, g: 100, b: 0 },   // dark orange
        Color::Rgb { r: 200, g: 50, b: 0 },    // red-orange
        Color::Rgb { r: 170, g: 20, b: 0 },    // dark red
        Color::Rgb { r: 139, g: 0, b: 0 },     // deep red
    ];
    for (i, line) in logo.iter().enumerate() {
        let col = term_cols.saturating_sub(logo_width) / 2;
        queue!(
            stdout,
            MoveTo(col, start_row + i as u16),
            SetForegroundColor(gradient[i]),
        )?;
        write!(stdout, "{line}")?;
    }

    // Status text below logo
    let status = format!("Waiting for other players to connect... ({}/{})", joined, needed);
    let status_col = term_cols.saturating_sub(status.len() as u16) / 2;
    queue!(
        stdout,
        MoveTo(status_col, start_row + logo_height + 2),
        SetForegroundColor(Color::White),
    )?;
    write!(stdout, "{status}")?;

    let player_info = format!("You are Player {}", player_id + 1);
    let info_col = term_cols.saturating_sub(player_info.len() as u16) / 2;
    queue!(
        stdout,
        MoveTo(info_col, start_row + logo_height + 3),
        SetForegroundColor(PLAYER_COLORS[player_id as usize % PLAYER_COLORS.len()]),
    )?;
    write!(stdout, "{player_info}")?;

    queue!(stdout, ResetColor)?;
    stdout.flush()?;
    Ok(())
}

#[tokio::main]
async fn main() -> io::Result<()> {
    let stream = TcpStream::connect("127.0.0.1:9000").await?;
    let (mut reader, mut writer) = tokio::io::split(stream);

    eprintln!("[client] Connected to server, waiting for initial message...");
    let msg: ServerMsg = recv_msg(&mut reader).await?;

    let player_id = match msg {
        ServerMsg::Welcome { player_id } => {
            eprintln!("[client] Joined as player {player_id}");
            player_id
        }
        other => {
            eprintln!("[client] Expected Welcome, got: {other:?}");
            return Ok(());
        }
    };

    // Enter raw mode and alternate screen
    let mut stdout = io::stdout();
    terminal::enable_raw_mode()?;
    execute!(stdout, EnterAlternateScreen, Hide)?;

    // Check for WaitingForPlayers
    let first_state: Option<ServerMsg> = {
        let msg: ServerMsg = recv_msg(&mut reader).await?;
        match msg {
            ServerMsg::WaitingForPlayers { joined, needed } => {
                eprintln!("[client] Waiting for other players...");
                display_lobby_screen(&mut stdout, player_id, joined, needed)?;
                None
            }
            other => Some(other),
        }
    };

    let result =
        run_game_loop(&mut reader, &mut writer, &mut stdout, player_id, first_state).await;

    // Cleanup
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
    first_msg: Option<ServerMsg>,
) -> io::Result<()> {
    let mut event_stream = EventStream::new();
    let mut renderer = Renderer::new();
    let mut last_state: Option<GameState> = None;
    let mut status_msg: Option<String> = None;

    // Process first message if we got one that wasn't WaitingForPlayers
    if let Some(msg) = first_msg {
        handle_server_msg(
            msg,
            stdout,
            &mut renderer,
            player_id,
            &mut last_state,
            &mut status_msg,
        )?;
    }

    loop {
        tokio::select! {
            maybe_event = event_stream.next() => {
                let Some(Ok(event)) = maybe_event else { break };
                if let Event::Key(KeyEvent { code, .. }) = event {
                    match code {
                        KeyCode::Up => {
                            send_msg(writer, &ClientMsg::Move(Direction::Up)).await?;
                        }
                        KeyCode::Down => {
                            send_msg(writer, &ClientMsg::Move(Direction::Down)).await?;
                        }
                        KeyCode::Left => {
                            send_msg(writer, &ClientMsg::Move(Direction::Left)).await?;
                        }
                        KeyCode::Right => {
                            send_msg(writer, &ClientMsg::Move(Direction::Right)).await?;
                        }
                        KeyCode::Char(' ') => {
                            send_msg(writer, &ClientMsg::PlaceBomb).await?;
                        }
                        KeyCode::Char('q') | KeyCode::Esc => break,
                        _ => {}
                    }
                }
            }
            result = recv_msg::<_, ServerMsg>(reader) => {
                let msg = result?;
                let should_quit = handle_server_msg(msg, stdout, &mut renderer, player_id, &mut last_state, &mut status_msg)?;
                if should_quit {
                    break;
                }
            }
        }
    }

    Ok(())
}

/// Handle a server message. Returns true if the game should quit.
fn handle_server_msg(
    msg: ServerMsg,
    stdout: &mut io::Stdout,
    renderer: &mut Renderer,
    player_id: u8,
    last_state: &mut Option<GameState>,
    status_msg: &mut Option<String>,
) -> io::Result<bool> {
    match msg {
        ServerMsg::State(state) => {
            renderer.render(stdout, &state, player_id, status_msg.as_deref())?;
            *last_state = Some(state);
        }
        ServerMsg::RoundStart { round } => {
            *status_msg = Some(format!("Round {round} - FIGHT!"));
        }
        ServerMsg::RoundOver { winner } => {
            let text = match winner {
                Some(w) if w == player_id => "Round Over - You win!".to_string(),
                Some(w) => format!("Round Over - Player {} wins!", w + 1),
                None => "Round Over - Draw!".to_string(),
            };
            *status_msg = Some(text);
        }
        ServerMsg::MatchOver { winner } => {
            let text = if winner == player_id {
                "MATCH OVER - YOU WIN! Press Q to quit.".to_string()
            } else {
                format!(
                    "MATCH OVER - Player {} wins! Press Q to quit.",
                    winner + 1
                )
            };
            *status_msg = Some(text.clone());
            if let Some(state) = &*last_state {
                renderer.render(stdout, state, player_id, Some(&text))?;
            }
        }
        ServerMsg::PlayerDisconnected { player_id: pid } => {
            *status_msg = Some(format!("Player {} disconnected", pid + 1));
        }
        ServerMsg::WaitingForPlayers { joined, needed } => {
            display_lobby_screen(stdout, player_id, joined, needed)?;
        }
        ServerMsg::Welcome { .. } => {}
    }
    Ok(false)
}
