import os
import sys
import time

# Enable ANSI escape sequences on Windows
if os.name == 'nt':
    os.system('')

# Define your isometric letter blocks
LETTER_m = [
    r"                ",
    r"                ",
    r" █████████████  ",
    r"▒▒███▒▒███▒▒███ ",
    r" ▒███ ▒███ ▒███ ",
    r" ▒███ ▒███ ▒███ ",
    r" █████▒███ █████",
    r"▒▒▒▒▒ ▒▒▒ ▒▒▒▒▒ "
]

LETTER_a = [
    r"          ",
    r"          ",
    r"  ██████  ",
    r" ▒▒▒▒▒███ ",
    r"  ███████ ",
    r" ███▒▒███ ",
    r"▒▒████████",
    r" ▒▒▒▒▒▒▒▒ ",
]

LETTER_n = [
    r"           ",
    r"           ",
    r" ████████  ",
    r"▒▒███▒▒███ ",
    r" ▒███ ▒███ ",
    r" ▒███ ▒███ ",
    r" ████ █████",
    r"▒▒▒▒ ▒▒▒▒▒ ",
]

LETTER_d = [
    r"     █████",
    r"    ▒▒███ ",
    r"  ███████ ",
    r" ███▒▒███ ",
    r"▒███ ▒███ ",
    r"▒███ ▒███ ",
    r"▒▒████████",
    r" ▒▒▒▒▒▒▒▒ ",
]

LETTER_i = [
    r"  ███ ",
    r" ▒▒▒  ",
    r" ████ ",
    r"▒▒███ ",
    r" ▒███ ",
    r" ▒███ ",
    r" █████",
    r"▒▒▒▒▒ ",
]

LETTER_b = [
    r" █████    ",
    r"▒▒███     ",
    r" ▒███████ ",
    r" ▒███▒▒███",
    r" ▒███ ▒███",
    r" ▒███ ▒███",
    r" ████████ ",
    r"▒▒▒▒▒▒▒▒  "
]

LETTER_l = [
    r" ████ ",
    r"▒▒███ ",
    r" ▒███ ",
    r" ▒███ ",
    r" ▒███ ",
    r" ▒███ ",
    r" █████",
    r"▒▒▒▒▒ "
]

LETTER_e = [
    r"         ",
    r"         ",
    r"  ██████ ",
    r" ███▒▒███",
    r"▒███████ ",
    r"▒███▒▒▒  ",
    r"▒▒██████ ",
    r" ▒▒▒▒▒▒  "
]

LETTERS = [LETTER_m, LETTER_a, LETTER_n, LETTER_d, LETTER_i, LETTER_b, LETTER_l, LETTER_e]

# --- Smooth Motion Configuration ---
# Trajectory array: 4 is baseline (bottom), 0 is peak height (top).
# Intermediate numbers (3, 2, 1) provide smooth sub-frame interpolation.
TRAJECTORY = [4, 3, 2, 1, 0, 0, 1, 2, 3, 4]
MAX_SHIFT = max(TRAJECTORY)

LETTER_HEIGHT = len(LETTERS[0])
CANVAS_HEIGHT = LETTER_HEIGHT + MAX_SHIFT
LETTER_SPACING = "  "

# Delay (in ticks) before the wave moves to the next letter
LETTER_DELAY = 2


def render_frame(offsets):
    """Renders the current frame from a list of Y-offsets for each letter."""
    frame_lines = []

    for row in range(CANVAS_HEIGHT):
        row_str = ""
        for i, letter in enumerate(LETTERS):
            y_offset = offsets[i]
            letter_width = len(letter[0])

            letter_row_idx = row - y_offset

            if 0 <= letter_row_idx < LETTER_HEIGHT:
                row_str += letter[letter_row_idx]
            else:
                row_str += " " * letter_width

            row_str += LETTER_SPACING

        frame_lines.append(row_str)

    return "\n".join(frame_lines)


def animate_smooth_wave(fps=20):
    delay = 1.0 / fps
    num_letters = len(LETTERS)
    
    # Total ticks per complete wave loop pass across all letters
    cycle_period = num_letters * LETTER_DELAY + len(TRAJECTORY)

    # Hide cursor & clear screen once
    sys.stdout.write("\033[?25l\033[2J")
    sys.stdout.flush()

    try:
        tick = 0
        while True:
            offsets = []

            for i in range(num_letters):
                # Calculate local trajectory step for letter `i` based on phase offset
                local_time = (tick - (i * LETTER_DELAY)) % cycle_period

                if 0 <= local_time < len(TRAJECTORY):
                    offsets.append(TRAJECTORY[local_time])
                else:
                    offsets.append(MAX_SHIFT)  # Baseline resting position

            # Draw frame to terminal home position (row 1, col 1)
            sys.stdout.write("\033[H")
            sys.stdout.write(render_frame(offsets) + "\n")
            sys.stdout.flush()

            tick += 1
            time.sleep(delay)

    except KeyboardInterrupt:
        pass
    finally:
        # Restore terminal cursor on exit
        sys.stdout.write("\033[?25h\033[0m\n")
        sys.stdout.flush()


if __name__ == "__main__":
    # Running at 20 FPS with 1-line steps makes the wave glide continuously
    animate_smooth_wave(fps=20)
