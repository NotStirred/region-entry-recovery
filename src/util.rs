use std::num::ParseIntError;
use crate::util::DuplicateBehaviour::{TakeCurrent, TakeUntracked};

pub const REGION_DIAMETER_IN_CHUNKS: u32 = 32;
pub const CHUNKS_PER_REGION: u32 = REGION_DIAMETER_IN_CHUNKS*REGION_DIAMETER_IN_CHUNKS;
pub const SECTOR_SIZE: usize = 4096;
pub const SECTOR_SIZE_BITS: u32 = 12;

pub const SIZE_BITS : u32 = 8;
pub const SIZE_MASK : u32 = (1 << SIZE_BITS) - 1;

#[derive(Clone, Copy, PartialEq)]
pub enum DuplicateBehaviour {
    TakeCurrent, // take the chunk referenced by the header file
    TakeUntracked, // take the chunk not referenced by the header file, choose if there are several
}

#[derive(Clone)]
pub struct RegionEntry {
    pub is_current: bool, // is the current entry referenced in the header
    pub offset_sectors: u32,
    pub size_sectors: u8,
}

pub fn chunk_position_from_entry_idx(region_position: (i32, i32), entry_idx: u16) -> (i32, i32) {
    let entry_x = entry_idx & 0x1f;
    let entry_z = entry_idx >> 5;

    return ((region_position.0 << 5) + (entry_x as i32), (region_position.1 << 5) + (entry_z as i32));
}

pub fn set_header_entry(bytes: &mut [u8], header_offset: usize, sector_idx: usize, size: u8) {
    bytes[header_offset + 3] = size;
    bytes[header_offset + 2] = ((sector_idx >> 0) & 0xff) as u8;
    bytes[header_offset + 1] = ((sector_idx >> 8) & 0xff) as u8;
    bytes[header_offset + 0] = ((sector_idx >> 16) & 0xff) as u8;

    let packed = read_bigendian_u32(bytes, header_offset);
    let written_offset = packed >> SIZE_BITS;
    let written_size = (packed & SIZE_MASK) as u8;

    assert_eq!(sector_idx as u32, written_offset);
    assert_eq!(size, written_size);
}

pub fn read_bigendian_u32(bytes: &[u8], header_offset: usize) -> u32 {
    (bytes[header_offset + 3] as u32) |
        (bytes[header_offset + 2] as u32) << 8 |
        (bytes[header_offset + 1] as u32) << 16 |
        (bytes[header_offset + 0] as u32) << 24
}

pub fn trim_newline(s: &mut String) {
    if s.ends_with('\n') {
        s.pop();
        if s.ends_with('\r') {
            s.pop();
        }
    }
}

pub fn ask_for_duplicate_behaviour_optional() -> Option<DuplicateBehaviour> {
    println!("If duplicate entries are found, what would you like to do?
    `DecidePerEntry` - Decide for each chunk each time there is a duplicate
    `TakeCurrent` - Always take the current chunk
    `TakeUntracked` - Always take one of the untracked chunks (you can decide if there are multiple)");

    loop {
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).unwrap();
        trim_newline(&mut line);

        match line.to_lowercase().as_str() {
            "decideperentry" => {
                break None;
            },
            "takecurrent" => {
                break Some(TakeCurrent);
            },
            "takeuntracked" => {
                break Some(TakeUntracked);
            },
            _ => {
                println!("Invalid value!");
            },
        }
    }
}
pub fn ask_for_duplicate_behaviour() -> DuplicateBehaviour {
    println!("Duplicate entries have been found, what would you like to do?
    `TakeCurrent` - Always take the current chunk
    `TakeUntracked` - Always take one of the untracked chunks (you can decide if there are multiple)");

    loop {
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).unwrap();
        trim_newline(&mut line);

        match line.to_lowercase().as_str() {
            "takecurrent" => {
                break TakeCurrent;
            },
            "takeuntracked" => {
                break TakeUntracked;
            },
            _ => {
                println!("Invalid value!");
            },
        }
    }
}

pub fn ask_for_integer_greater_than_1() -> u32 {
    loop {
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).unwrap();
        trim_newline(&mut line);

        let parsed: Result<i32, ParseIntError> = line.parse();
        match parsed {
            Ok(value) => {
                if value > 1 {
                    return value as u32
                }
            }
            Err(_) => {
                println!("Invalid value!");
            }
        }
    }
}
