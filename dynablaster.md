# DYNABLASTER -- Battle Mode Clone Reference

## 1. OVERVIEW

Dyna Blaster (1992) is the European localization of Hudson Soft's Bomberman. We are cloning the **multiplayer Battle Mode** -- a competitive last-man-standing deathmatch on a grid-based arena.

---

## 2. CORE GAMEPLAY MECHANICS

### Grid / Tile System
- The playfield is a **rectangular grid** of tiles, viewed from a top-down perspective.
- **Standard grid dimensions:** 15 columns x 13 rows.
- The outer perimeter is lined with indestructible border walls, so the playable interior is 13x11 tiles.
- Each tile is one of:
  - **Empty/Floor:** Passable space where players can move.
  - **Hard Block (Indestructible Wall):** Permanent walls arranged in a regular lattice pattern inside the border.
  - **Soft Block (Destructible/Brick Wall):** Randomly placed breakable walls that can be destroyed by bomb explosions. These hide power-ups.

### Movement
- Players move in **4 directions** (up, down, left, right) on the grid.
- Movement is tile-aligned but smooth -- the character animates between tiles.
- Default movement speed is relatively slow; it can be increased with Speed Up power-ups.
- Players **cannot** move through hard blocks, soft blocks, or bombs.
- Movement is **grid-snapped**: when you press a direction, the character aligns to the grid lane.

### Corner Rounding / Tile Alignment Assist
- The player has a slightly smaller hitbox than a full tile, allowing them to "slide" around corners. If a player is moving horizontally and slightly misaligned with a corridor opening, the game nudges them into alignment. This is crucial for the game feeling responsive and not frustrating.

### Bomb Placement
- Pressing the action button places a bomb on the tile the player is currently standing on.
- The player starts with the ability to place **1 bomb** at a time.
- After placing a bomb, the player can walk off it, but **cannot re-enter** the bomb's tile (the bomb acts as a solid obstacle).
- Bombs have a **fuse timer of approximately 2.5 seconds** before detonating.
- Multiple bombs can be placed simultaneously if the player has collected Bomb Up items.
- Only one bomb can occupy a tile at a time.

### Explosions
- When a bomb detonates, it produces a **cross-shaped explosion** extending in 4 cardinal directions (up, down, left, right) from the bomb's tile.
- The default blast radius is **1 tile** in each direction (the explosion covers the bomb tile plus 1 tile in each direction).
- Fire Up power-ups increase the blast radius by 1 tile per pickup.
- Explosions are **stopped by hard blocks** -- they do not pass through.
- Explosions **destroy soft blocks** but stop at the first soft block hit in each direction (the soft block is destroyed but the explosion does not continue past it).
- Explosions **kill players** on contact (players can be killed by their own bombs or other players' bombs).
- **Chain reactions:** If an explosion reaches another bomb, that bomb detonates immediately, creating a chain reaction. This is a fundamental strategic mechanic.
- Explosions are momentary -- they appear for a brief visual duration (about 0.5 seconds) and then disappear.
- Explosion flames appear on each tile in the blast path, including a distinct "center" piece and "tip" pieces at the ends.

---

## 3. POWER-UPS

Power-ups are hidden under soft blocks. When a soft block covering a power-up is destroyed, the item becomes visible and can be collected by walking over it. If an explosion hits an exposed power-up, the item is **destroyed permanently**.

| Item | Effect |
|------|--------|
| **Bomb Up** | Increases the number of bombs the player can place simultaneously by 1. Cumulative. |
| **Fire Up** (Flame Up) | Increases bomb blast radius by 1 tile in each direction. Cumulative. |
| **Speed Up** | Increases player movement speed by one increment. Cumulative (but too many can make control difficult). |
| **Full Fire** (Maximum Fire) | Instantly maximizes the blast radius to the full width/height of the arena. Very rare. |

### Power-Up Stacking Limits
- **Bomb Up:** Maximum of approximately 8-10 bombs simultaneously.
- **Fire Up:** Maximum blast radius extends to the edge of the arena (effectively 7-8 tiles in each direction, limited by hard blocks).
- **Speed Up:** Maximum of approximately 4-5 speed increments. Beyond this, movement becomes too fast to control effectively.

---

## 4. BATTLE MODE RULES

### Basic Setup
- **2 to 5 players** compete in a last-man-standing deathmatch.
- Players compete across **multiple rounds** (configurable, typically best of 3 or 5).
- All players start with **default stats:** 1 bomb, 1-tile blast radius, normal speed.

### Battle Arena
- 15x13 grid with hard blocks in the standard lattice pattern.
- Soft blocks fill most remaining spaces at the start (~60-70% of available floor tiles).
- **Player starting positions:** Each player starts in one of the corners/edges of the arena:
  - Player 1: Top-left corner
  - Player 2: Top-right corner
  - Player 3: Bottom-left corner
  - Player 4: Bottom-right corner
  - Player 5: Center-top or center-bottom (depending on version)
  - Each starting position has 2-3 tiles cleared of soft blocks to prevent immediate trapping.

### Round Flow
1. Arena is generated with soft blocks and hidden power-ups.
2. Players place bombs to destroy soft blocks, collect power-ups, and eliminate opponents.
3. The last player standing wins the round.
4. A player killed by any bomb explosion is eliminated for that round.
5. The match winner is the player who accumulates the target number of round victories first.

### Sudden Death / Arena Shrinking
- Each round has a time limit (typically 2-3 minutes).
- When the timer runs out, hard blocks begin falling from the perimeter inward.
- They drop one row/column at a time, spiraling inward.
- Any player caught under a falling block is instantly killed.
- This prevents stalemates and forces players toward the center for a final confrontation.

---

## 5. GRID / TILE SYSTEM TECHNICAL DETAILS

### Hard Block Pattern
- Inside the border, hard blocks are placed in a **regular grid pattern** at positions where both the row and column are even (0-indexed within the inner play area):
  - At every intersection where (column % 2 == 0) AND (row % 2 == 0), place a hard block.
  - This creates a pillar pattern with corridors between them.
  - The result is alternating rows of: `[floor, floor, floor, floor...]` and `[floor, HARD, floor, HARD...]`

### Tile Types Summary
1. **Border Wall** -- Indestructible, lines the outside edge
2. **Hard Block / Pillar** -- Indestructible, placed in the regular interior pattern
3. **Soft Block / Brick** -- Destructible, randomly placed on available floor tiles
4. **Floor** -- Empty traversable space
5. **Power-Up** -- Hidden under a soft block, revealed when that block is destroyed
6. **Bomb** -- Placed by players, acts as temporary solid obstacle
7. **Explosion** -- Temporary, occupies tiles during detonation

### Coordinate System
- Tile-based coordinate system where each game entity occupies or moves between discrete tile positions.
- Player movement is interpolated between tiles for smooth animation but logically snaps to the grid for collision detection and bomb placement.

---

## 6. OTHER NOTABLE MECHANICS

### Bomb Kicking (NOT in scope)
- Bomb kicking, punching, and throwing were introduced in later Bomberman titles. The original Dyna Blaster does **not** have these mechanics.

### Bomb Stacking
- Only one bomb can occupy a tile at a time. Players cannot place a bomb on a tile that already has a bomb.

### Controls
- **D-Pad / Arrow Keys:** 4-directional movement
- **Action Button:** Place bomb
- **Start/Pause:** Pause game