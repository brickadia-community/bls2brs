use brs::{chrono::prelude::*, uuid::Uuid};
use std::{
    collections::HashMap,
    convert::TryInto,
    io::{self, prelude::*},
    ops::Neg,
};

pub use bl_save;
pub use brs;

mod types;
#[macro_use]
mod misc;
mod mappings;

use mappings::{BRICK_MAP_LITERAL, BRICK_MAP_REGEX};
use types::{BrickDesc, BrickMapping};

// Keep this in sync. Would be nice to just determine the indices at compile time.
const FIXED_MATERIAL_TABLE: &[&str] = &["BMC_Plastic", "BMC_Glow", "BMC_Metallic"];
const BMC_PLASTIC: usize = 0;
const BMC_GLOW: usize = 1;
const BMC_METALLIC: usize = 2;

const BRICK_OWNER: usize = 0;

pub struct ConvertReport {
    pub write_data: brs::WriteData,
    pub unknown_ui_names: HashMap<String, usize>,
    pub count_success: usize,
    pub count_failure: usize,
}

pub fn convert(reader: bl_save::Reader<impl BufRead>) -> io::Result<ConvertReport> {
    let data = brs::WriteData {
        map: String::from("Unknown"),
        author: brs::User {
            id: Uuid::nil(),
            name: String::from("Unknown"),
        },
        description: reader.description().to_string(),
        save_time: Utc::now(),
        mods: vec![],
        brick_assets: vec![],
        colors: reader.colors().iter().map(|c| map_color(*c)).collect(),
        materials: FIXED_MATERIAL_TABLE
            .iter()
            .map(|s| String::from(*s))
            .collect(),
        brick_owners: vec![brs::User {
            id: Uuid::from_bytes([u8::max_value(); 16]),
            name: String::from("PUBLIC"),
        }],
        bricks: Vec::with_capacity(reader.brick_count().unwrap_or(100).min(10_000_000)),
    };

    let mut converter = Converter {
        write_data: data,
        asset_map: HashMap::new(),
        unknown_ui_names: HashMap::new(),
    };

    let mut count_success = 0;
    let mut count_failure = 0;

    let mut non_prio = Vec::new();

    for from in reader {
        let from = from?;
        let option = converter.map_brick(&from);

        let mappings = match option {
            Some(mappings) => {
                count_success += 1;
                mappings
            }
            None => {
                count_failure += 1;
                continue;
            }
        };

        // match from.base.print.as_str() {
        //     "A" => {
        //
        //     }
        //     _ => {}
        // }

        for BrickDesc {
            asset,
            mut size,
            offset,
            rotation_offset,
            color_override,
            mut direction_override,
            non_priority,
            microwedge_rotate,
            inverted_modter_rotate,
            inverted_wedge_rotate,
            modter,
            lattice_rotate,
            nocollide
        } in mappings
        {
            let asset_name_index = converter.asset(asset);
            let mut rotation = (from.base.angle + rotation_offset) % 4;

            let rotated_xy = rotate_offset((offset.0, offset.1), from.base.angle);
            let offset = (rotated_xy.0, rotated_xy.1, offset.2);

            let position = (
                (from.base.position.1 * 20.0) as i32 + offset.0,
                (from.base.position.0 * 20.0) as i32 + offset.1,
                (from.base.position.2 * 20.0) as i32 + offset.2,
            );

            let material_index = match from.base.color_fx {
                3 => BMC_GLOW,
                1 | 2 => BMC_METALLIC,
                _ => BMC_PLASTIC,
            };

            let color_index = match color_override {
                Some(color) => converter.color(color) as u32,
                None => u32::from(from.base.color_index),
            };

            // convert a vertical slope to microwedge
            if microwedge_rotate {
                let original_dir = direction_override;
                let (x, y, z) = size;
                if rotation == 0 || rotation == 2 {
                    direction_override = Some(brs::Direction::YPositive);
                    if rotation == 0 {
                        size = (z, x, y);
                    } else {
                        size = (x, z, y);
                        rotation = (rotation + 1) % 4;
                    }
                } else {
                    direction_override = Some(brs::Direction::XPositive);
                    if rotation == 1 {
                        size = (x, z, y);
                        rotation = (rotation + 2) % 4;
                    } else {
                        size = (z, x, y);
                        rotation = (rotation + 1) % 4;
                    }
                }
                if original_dir.is_some() && original_dir.unwrap() == brs::Direction::ZNegative {
                    rotation = (rotation + 2) % 4;
                }
            }

            if lattice_rotate {
                if rotation == 0 || rotation == 2 {
                    direction_override = if rotation == 0 {
                        Some(brs::Direction::YPositive)
                    } else {
                        Some(brs::Direction::YNegative)
                    };
                    let (x, y, z) = size;
                    size = (y, x, z);
                } else {
                    direction_override = if rotation == 1 {
                        Some(brs::Direction::XNegative)
                    } else {
                        Some(brs::Direction::XPositive)
                    };
                }
            }

            // fix odd rotation offsets on inverted ModTer, wedges
            if (inverted_modter_rotate && (rotation == 1 || rotation == 3)) ||
                (inverted_wedge_rotate && (rotation == 0 || rotation == 2)) {
                rotation = (rotation + 2) % 4;
            }

            let color = if let Some(c) = color_override {
                brs::ColorMode::Custom(c)
            } else {
                brs::ColorMode::Set(color_index)
            };

            let collision = if from.base.collision {
                !nocollide
            } else {
                false
            };

            let brick = brs::Brick {
                asset_name_index: asset_name_index as u32,
                size,
                position,
                direction: direction_override.unwrap_or(brs::Direction::ZPositive),
                rotation: rotation.try_into().unwrap(),
                collision,
                visibility: from.base.rendering,
                material_index: material_index as u32,
                color,
                owner_index: BRICK_OWNER as u32,
            };

            if non_priority || (modter && !brick.visibility) {
                non_prio.push(brick);
            } else {
                converter.write_data.bricks.push(brick);
            }
        }
    }
    
    converter.write_data.bricks.append(&mut non_prio);

    Ok(ConvertReport {
        write_data: converter.write_data,
        unknown_ui_names: converter.unknown_ui_names,
        count_success,
        count_failure,
    })
}

struct Converter {
    write_data: brs::WriteData,
    asset_map: HashMap<String, usize>,
    unknown_ui_names: HashMap<String, usize>,
}

impl Converter {
    fn map_brick(&mut self, from: &bl_save::Brick) -> Option<BrickMapping> {
        let mapping = map_brick(from);

        if cfg!(debug_assertions) {
            println!("mapped '{}' to {:?}", from.base.ui_name, mapping);
        }

        if mapping.is_none() {
            *self
                .unknown_ui_names
                .entry(from.base.ui_name.clone())
                .or_default() += 1;
        }

        mapping
    }

    fn asset(&mut self, asset_name: &str) -> usize {
        if let Some(index) = self.asset_map.get(asset_name) {
            return *index;
        }

        let index = self.write_data.brick_assets.len();
        self.write_data.brick_assets.push(asset_name.to_string());
        self.asset_map.insert(asset_name.to_string(), index);

        index
    }

    fn color(&mut self, color: brs::Color) -> usize {
        // TODO: Optimize lookup with a map
        for (index, other) in self.write_data.colors.iter().enumerate() {
            if *other == color {
                return index;
            }
        }

        let index = self.write_data.colors.len();
        self.write_data.colors.push(color);
        index
    }
}

fn map_brick(from: &bl_save::Brick) -> Option<BrickMapping> {
    let ui_name = from.base.ui_name.as_str();

    if let Some(mapping) = BRICK_MAP_LITERAL.get(ui_name) {
        return Some(mapping.clone());
    }

    for (regex, func) in BRICK_MAP_REGEX.iter() {
        if let Some(captures) = regex.captures(ui_name) {
            return func(captures, from);
        }
    }

    None
}

fn map_color((r, g, b, a): (f32, f32, f32, f32)) -> brs::Color {
    // Convert into Unreal color space
    let r = gamma_expansion(r);
    let g = gamma_expansion(g);
    let b = gamma_expansion(b);
    let a = gamma_expansion(a);

    // Convert to 0-255
    let r = (r * 255.0).max(0.0).min(255.0) as u8;
    let g = (g * 255.0).max(0.0).min(255.0) as u8;
    let b = (b * 255.0).max(0.0).min(255.0) as u8;
    let a = (a * 255.0).max(0.0).min(255.0) as u8;

    brs::Color::from_rgba(r, g, b, a)
}

fn gamma_expansion(u: f32) -> f32 {
    if u <= 0.04045 {
        return u / 12.92;
    }
    let base = (u + 0.055) / 1.055;
    base.powf(2.4)
}

fn rotate_offset(mut offset: (i32, i32), angle: u8) -> (i32, i32) {
    for _ in 0..angle {
        offset = rotate_90_2d(offset);
    }
    offset
}

fn rotate_90_2d<X, Y: Neg>((x, y): (X, Y)) -> (<Y as Neg>::Output, X) {
    (-y, x)
}
