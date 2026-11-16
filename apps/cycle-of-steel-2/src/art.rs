//! Placeholder pixel art, drawn in code and packed into one texture.
//!
//! The feel prototype needs a handful of sprites and a digit font, and drawing
//! them here keeps it free of an asset pipeline until the art pass. Each
//! sprite is rows of palette letters, 16 pixels to a tile. Every sprite also
//! gets a white silhouette, used for hit flashes.

/// One letter per colour; `.` is clear.
const PALETTE: &[(u8, [u8; 4])] = &[
    (b'k', [5, 7, 10, 255]),
    (b'd', [93, 102, 112, 255]),
    (b'g', [154, 163, 173, 255]),
    (b'G', [211, 217, 222, 255]),
    (b'w', [255, 255, 255, 255]),
    (b'y', [201, 143, 46, 255]),
    (b'Y', [243, 210, 122, 255]),
    (b'r', [216, 56, 42, 255]),
    (b'R', [255, 176, 160, 255]),
    (b'b', [107, 63, 34, 255]),
    (b'B', [168, 105, 61, 255]),
    (b'c', [201, 143, 90, 255]),
    (b's', [0, 0, 0, 90]),
    (b'o', [236, 120, 40, 255]),
    (b't', [60, 72, 88, 255]),
    (b'T', [96, 116, 140, 255]),
];

/// The sprites, in atlas order; [`Art`] names their indices.
const SPRITES: &[&[&str]] = &[
    // White: one pixel, for tinted squares.
    &["w"],
    // Player: a small engineer in goggles, facing right.
    &[
        "...kkkkk....",
        "..kTTTTTk...",
        ".kTTTTTTTk..",
        ".kkkkkkkkkk.",
        ".kccckYYkYk.",
        ".kcccckkkkk.",
        "..kcccccck..",
        "..ktttttk...",
        ".kttTTtttk..",
        ".ktTTttttk..",
        ".kttttttk...",
        "..ktktktk...",
        "..kbk.kbk...",
        "..kkk.kkk...",
    ],
    // Gun: a stubby pistol pointing right; the grip is at the left.
    &[
        "..kkkkkkkkkk",
        ".kGGGGGGGGGk",
        ".kgggggggggk",
        ".kdkkkkkkkkk",
        "kddk.kk.....",
        "kddk........",
        "kkkk........",
    ],
    // Training dummy.
    &[
        "...kkkkkk...",
        "..kBBBBBBk..",
        ".kBccccccBk.",
        ".kBcrrrrcBk.",
        ".kBcrwwrcBk.",
        ".kBcrrrrcBk.",
        ".kBccccccBk.",
        "..kBBBBBBk..",
        "kkkkbbbbkkkk",
        "kBBBBBBBBBBk",
        "kkkkbbbbkkkk",
        "...kBccBk...",
        "...kBccBk...",
        "...kBccBk...",
        "....kbbk....",
        "....kbbk....",
        "...kbbbbk...",
        "..kkkkkkkk..",
    ],
    // Bullet, flying right.
    &[".YYw.", "YYYww", ".YYw."],
    // Muzzle flash, pointing right.
    &[
        "..w....",
        "...ww..",
        "wwwwwYw",
        ".wYYYww",
        "wwwwwYw",
        "...ww..",
        "..w....",
    ],
    // Shadow under things that stand on the floor.
    &[".ssssssss.", "ssssssssss", ".ssssssss."],
];

/// Digits 0 to 9, three pixels wide and five high, as rows of bits.
const DIGITS: [[u8; 5]; 10] = [
    [0b111, 0b101, 0b101, 0b101, 0b111],
    [0b010, 0b110, 0b010, 0b010, 0b111],
    [0b111, 0b001, 0b111, 0b100, 0b111],
    [0b111, 0b001, 0b011, 0b001, 0b111],
    [0b101, 0b101, 0b111, 0b001, 0b001],
    [0b111, 0b100, 0b111, 0b001, 0b111],
    [0b111, 0b100, 0b111, 0b101, 0b111],
    [0b111, 0b001, 0b010, 0b010, 0b010],
    [0b111, 0b101, 0b111, 0b101, 0b111],
    [0b111, 0b101, 0b111, 0b001, 0b111],
];
pub const DIGIT_SIZE: [u32; 2] = [3, 5];
/// Pixels per tile.
pub const PIXELS_PER_TILE: f32 = 16.0;

/// Names for the sprites' places in the atlas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Art {
    White,
    Player,
    Gun,
    Dummy,
    Bullet,
    Muzzle,
    Shadow,
}

#[derive(Clone, Copy, Debug)]
pub struct Region {
    /// `[u0, v0, u1, v1]` in the atlas.
    pub uv: [f32; 4],
    /// Size in tiles.
    pub size: [f32; 2],
}

pub struct Atlas {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// Per sprite: the sprite, then its white silhouette.
    sprites: Vec<(Region, Region)>,
    digits: Vec<Region>,
}

impl Atlas {
    pub fn region(&self, art: Art) -> Region {
        self.sprites[art as usize].0
    }

    pub fn silhouette(&self, art: Art) -> Region {
        self.sprites[art as usize].1
    }

    pub fn digit(&self, digit: u32) -> Region {
        self.digits[digit as usize % 10]
    }

    /// Packs everything into one row-wrapped texture with a pixel of space between.
    pub fn build() -> Self {
        let mut images: Vec<(u32, u32, Vec<u8>)> = Vec::new();
        for rows in SPRITES {
            let image = sprite_image(rows);
            let silhouette = whiten(&image);
            images.push(image);
            images.push(silhouette);
        }
        for bits in DIGITS {
            let mut rgba = Vec::new();
            for row in bits {
                for column in (0..3).rev() {
                    rgba.extend_from_slice(if row >> column & 1 == 1 { &[255; 4] } else { &[0; 4] });
                }
            }
            images.push((DIGIT_SIZE[0], DIGIT_SIZE[1], rgba));
        }

        const WIDTH: u32 = 128;
        let mut places = Vec::with_capacity(images.len());
        let (mut x, mut y, mut row_height) = (1, 1, 0);
        for (w, h, _) in &images {
            if x + w + 1 > WIDTH {
                x = 1;
                y += row_height + 1;
                row_height = 0;
            }
            places.push((x, y));
            x += w + 1;
            row_height = row_height.max(*h);
        }
        let height = (y + row_height + 1).next_power_of_two();
        let mut rgba = vec![0u8; (WIDTH * height * 4) as usize];
        let mut regions = Vec::with_capacity(images.len());
        for ((w, h, pixels), (px, py)) in images.iter().zip(&places) {
            for row in 0..*h {
                let from = (row * w * 4) as usize;
                let to = (((py + row) * WIDTH + px) * 4) as usize;
                rgba[to..to + (w * 4) as usize].copy_from_slice(&pixels[from..from + (w * 4) as usize]);
            }
            let (fw, fh) = (WIDTH as f32, height as f32);
            regions.push(Region {
                uv: [*px as f32 / fw, *py as f32 / fh, (px + w) as f32 / fw, (py + h) as f32 / fh],
                size: [*w as f32 / PIXELS_PER_TILE, *h as f32 / PIXELS_PER_TILE],
            });
        }
        let sprite_count = SPRITES.len();
        let sprites = (0..sprite_count).map(|i| (regions[2 * i], regions[2 * i + 1])).collect();
        let digits = regions[2 * sprite_count..].to_vec();
        Self {
            width: WIDTH,
            height,
            rgba,
            sprites,
            digits,
        }
    }
}

fn sprite_image(rows: &[&str]) -> (u32, u32, Vec<u8>) {
    let width = rows.iter().map(|row| row.len()).max().unwrap_or(0) as u32;
    let mut rgba = Vec::with_capacity((width * rows.len() as u32 * 4) as usize);
    for row in rows {
        for i in 0..width as usize {
            let letter = row.as_bytes().get(i).copied().unwrap_or(b'.');
            let color = PALETTE.iter().find(|(key, _)| *key == letter).map_or([0; 4], |(_, color)| *color);
            rgba.extend_from_slice(&color);
        }
    }
    (width, rows.len() as u32, rgba)
}

fn whiten(image: &(u32, u32, Vec<u8>)) -> (u32, u32, Vec<u8>) {
    let rgba = image.2.chunks_exact(4).flat_map(|p| if p[3] > 0 { [255, 255, 255, p[3]] } else { [0; 4] }).collect();
    (image.0, image.1, rgba)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_letter_is_in_the_palette() {
        for rows in SPRITES {
            for row in *rows {
                for letter in row.bytes() {
                    assert!(letter == b'.' || PALETTE.iter().any(|(key, _)| *key == letter), "unknown {}", letter as char);
                }
            }
        }
    }

    #[test]
    fn regions_sit_inside_the_atlas_and_keep_pixel_sizes() {
        let atlas = Atlas::build();
        assert_eq!(atlas.rgba.len(), (atlas.width * atlas.height * 4) as usize);
        let dummy = atlas.region(Art::Dummy);
        assert_eq!(dummy.size, [12.0 / 16.0, 18.0 / 16.0]);
        for region in [atlas.region(Art::Player), atlas.silhouette(Art::Dummy), atlas.digit(9)] {
            assert!(region.uv.iter().all(|v| (0.0..=1.0).contains(v)));
            assert!(region.uv[0] < region.uv[2] && region.uv[1] < region.uv[3]);
        }
    }
}
