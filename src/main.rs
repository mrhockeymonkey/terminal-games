use std::io::{self, Write};

use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{read, Event, KeyCode, KeyEvent},
    execute,
    terminal::{
        self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen,
    },
};

fn main() -> io::Result<()> {
    let mut stdout = io::stdout();

    terminal::enable_raw_mode()?;
    execute!(stdout, EnterAlternateScreen, Hide)?;

    let (width, height) = terminal::size()?;
    let mut player_x = width / 2;
    let mut player_y = height / 2;

    loop {
        // Render
        execute!(
            stdout,
            Clear(ClearType::All),
            MoveTo(player_x, player_y),
        )?;
        write!(stdout, "X")?;
        stdout.flush()?;

        // Input
        let event = read()?;
        if let Event::Key(KeyEvent { code, .. }) = event {
            match code {
                KeyCode::Up => player_y = player_y.saturating_sub(1),
                KeyCode::Down => {
                    if player_y < height - 1 {
                        player_y += 1;
                    }
                }
                KeyCode::Left => player_x = player_x.saturating_sub(1),
                KeyCode::Right => {
                    if player_x < width - 1 {
                        player_x += 1;
                    }
                }
                KeyCode::Char('q') | KeyCode::Esc => break,
                _ => {}
            }
        }
    }

    // Cleanup
    execute!(stdout, Show, LeaveAlternateScreen)?;
    terminal::disable_raw_mode()?;

    Ok(())
}
