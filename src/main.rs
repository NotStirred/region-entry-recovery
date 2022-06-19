use std::fs;
use std::io::{BufReader, Error};
use std::path::Path;

use bit_set::BitSet;

use quartz_nbt::io::{Flavor};
use quartz_nbt::{NbtTag};

const CHUNKS_PER_REGION: usize = 1024;
const SECTOR_SIZE: usize = 4096;
const SECTOR_SIZE_BITS: usize = 12;

const SIZE_BITS : usize = 8;
const SIZE_MASK : usize = (1 << SIZE_BITS) - 1;

fn recover_entries(file_name: &str) -> Result<(), Error> {
    let mut bytes = fs::read(file_name)?;

    let mut recovery_candidates = Vec::new();

    let sector_count = bytes.len() / SECTOR_SIZE;

    let mut occupied_sectors = BitSet::with_capacity(sector_count);

    occupied_sectors.insert(0); //first two sectors are used by header & timestamp header
    occupied_sectors.insert(1);


    for chunk_idx in 0..CHUNKS_PER_REGION {
        let sector_offset = (chunk_idx * 4) as usize;

        let packed = read_bigendian_usize(&bytes, sector_offset);

        if packed == 0 {
            recovery_candidates.push(chunk_idx);
            continue;
        }

        let offset = packed >> SIZE_BITS;
        let size = packed & SIZE_MASK;
        if offset == 0 || size == 0 || offset > sector_count { // invalid header data
            continue;
        }

        for sector_idx in offset..offset+size { // mark these parsed sectors as used
            occupied_sectors.insert(sector_idx);
        }
    }

    match rediscover_lost_entries(&mut bytes, occupied_sectors) {
        Ok(found_entries) => {
            if found_entries {
                println!("Writing to region file {}", file_name);
                fs::write(file_name, bytes)?;
            } else {
                println!("Found no entries for region file {}", file_name)
            }
        }
        Err(err) => {
            println!("Error while parsing region file {}", err);
        }
    }

    Ok(())
}

fn rediscover_lost_entries(bytes: &mut [u8], occupied_sectors: BitSet) -> Result<bool, Error> {
    let mut found_any_entries = false;
    for sector_idx in 2..bytes.len()/SECTOR_SIZE {
        if occupied_sectors.contains(sector_idx) {
            continue; //sector is already occupied
        }

        let byte_offset = sector_idx * SECTOR_SIZE;
        let size_bytes = read_bigendian_usize(bytes, byte_offset);

        let compression_format = bytes[byte_offset + 4];
        if size_bytes > bytes.len() - byte_offset || (compression_format != 1 && compression_format != 2) {
            // size or format are invalid, skip
            continue;
        }
        let compression_format = if compression_format == 1 { Flavor::GzCompressed } else { Flavor::ZlibCompressed };
        let sector_size = f32::ceil(size_bytes as f32 / SECTOR_SIZE as f32) as u8;

        // we now have a valid size and format, try to decompress
        let slice_start = byte_offset + 4 + 1; // skip the size and format bytes
        let slice_end = slice_start + size_bytes;

        let root = quartz_nbt::io::read_nbt(&mut BufReader::new(&mut std::io::Cursor::new(
            &bytes[slice_start..slice_end])), compression_format);

        let mut chunk_x = None;
        let mut chunk_z = None;

        let mut found_entries = false;
        match root {
            Ok(value) => {
                let level = if value.0.contains_key("Level") {
                    match value.0.get::<_, &NbtTag>("Level").unwrap() {
                        NbtTag::Compound(t) => { t },
                        _ => {
                            println!("Found valid compressed entry, but no level tag was found?!");
                            continue
                        }
                    }
                } else {
                    &value.0
                };

                if let NbtTag::Int(value) = level.get::<_, &NbtTag>("xPos").unwrap() {
                    chunk_x = Some(*value)
                };
                if let NbtTag::Int(value) = level.get::<_, &NbtTag>("zPos").unwrap() {
                    chunk_z = Some(*value)
                };

                let chunk_x = chunk_x.unwrap();
                let chunk_z = chunk_z.unwrap();
                let header_offset = ((chunk_x & 0x1f) + ((chunk_z & 0x1f) << 5)) as usize;

                println!("Recovered unknown region entry at chunk position ({}, {})!", chunk_x, chunk_z);
                let existing_packed = read_bigendian_usize(bytes, header_offset*4);
                let existing_offset = existing_packed >> SIZE_BITS;
                println!("Existing header entry points to {}, found entry at {}", existing_offset, sector_idx);
                found_entries = true;
            }
            Err(_) => {
                println!("Ignoring invalid entry");
            }
        }

        if found_entries {
            let chunk_x = chunk_x.unwrap();
            let chunk_z = chunk_z.unwrap();
            let header_offset = ((chunk_x & 0x1f) + ((chunk_z & 0x1f) << 5)) as usize;

            set_header_entry(bytes, header_offset*4, sector_idx, sector_size);
        }
        found_any_entries |= found_entries;
    }
    Ok(found_any_entries)
}

fn set_header_entry(bytes: &mut [u8], header_offset: usize, sector_idx: usize, size: u8) {
    bytes[header_offset + 3] = size;
    bytes[header_offset + 2] = ((sector_idx >> 0) & 0xff) as u8;
    bytes[header_offset + 1] = ((sector_idx >> 8) & 0xff) as u8;
    bytes[header_offset + 0] = ((sector_idx >> 16) & 0xff) as u8;

    let packed = read_bigendian_usize(bytes, header_offset);
    let written_offset = packed >> SIZE_BITS;
    let written_size = (packed & SIZE_MASK) as u8;

    assert_eq!(sector_idx, written_offset);
    assert_eq!(size, written_size);
}

fn read_bigendian_usize(bytes: &[u8], header_offset: usize) -> usize {
    (bytes[header_offset + 3] as usize) |
    (bytes[header_offset + 2] as usize) << 8 |
    (bytes[header_offset + 1] as usize) << 16 |
    (bytes[header_offset + 0] as usize) << 24
}

fn main() -> std::io::Result<()> {
    let mut world_path = "/the/world/path".to_owned();
    if !world_path.ends_with('/') {
        world_path += "/";
    }

    world_path += "region/";

    let world_path = Path::new(&world_path);
    for entry in fs::read_dir(world_path)?{
        let entry = entry?;
        let entry_path = entry.path();
        if !entry_path.is_dir() {
            let extension = entry_path.extension();
            if let Some(ext) = extension {
                if ext.to_str().unwrap().ends_with("mca") {
                    match recover_entries(entry.path().to_str().unwrap()) {
                        Ok(_) => {}
                        Err(err) => {
                            println!("Error parsing region file {}", err);
                        }
                    }
                }
            }
        }
    }

    Ok(())
}
