mod util;

use std::fs;
use std::io::{BufReader, Error};
use std::path::{Path, PathBuf};

use quartz_nbt::io::Flavor;
use quartz_nbt::NbtTag;

use clap::Parser;

use crate::util::{
    ask_for_duplicate_behaviour, ask_for_integer, chunk_position_from_entry_idx,
    read_bigendian_u32, set_header_entry, DuplicateBehaviour, RegionEntry, SECTOR_SIZE, SIZE_BITS,
    SIZE_MASK,
};
use crate::DuplicateBehaviour::{TakeCurrent, TakeUntracked};

/// Look through this byte array for valid region entries, return them all in a collection where the outer Vec is indexed by header idx
/// and the inner one contains all entries that match that position
fn discover_all_entries(bytes: &[u8]) -> Vec<Vec<RegionEntry>> {
    let mut discovered_entries: Vec<Vec<RegionEntry>> = vec![Vec::new(); SECTOR_SIZE];

    for sector_idx in 2..bytes.len() / SECTOR_SIZE {
        let byte_offset = sector_idx * SECTOR_SIZE;
        let size_bytes = read_bigendian_u32(bytes, byte_offset) as usize;

        let compression_format = bytes[byte_offset + 4];
        if size_bytes > bytes.len() - byte_offset
            || (compression_format != 1 && compression_format != 2)
        {
            // size or format are invalid, skip
            continue;
        }
        let compression_format = if compression_format == 1 {
            Flavor::GzCompressed
        } else {
            Flavor::ZlibCompressed
        };
        let size_sectors = f32::ceil(size_bytes as f32 / SECTOR_SIZE as f32) as u8;

        // we now have a valid size and format, try to decompress
        let slice_start = byte_offset + 4 + 1; // skip the size and format bytes
        let slice_end = slice_start + size_bytes;

        //attempt to parse this possible entry as nbt
        let root = quartz_nbt::io::read_nbt(
            &mut BufReader::new(&mut std::io::Cursor::new(&bytes[slice_start..slice_end])),
            compression_format,
        );

        if let Ok(value) = root {
            // in some earlier versions the Level tag was used, later versions dropped it
            let level = if value.0.contains_key("Level") {
                match value.0.get::<_, &NbtTag>("Level").unwrap() {
                    NbtTag::Compound(t) => t,
                    _ => {
                        println!("Found valid compressed entry, but no level tag was found?!");
                        continue;
                    }
                }
            } else {
                &value.0
            };

            let chunk_x = if let NbtTag::Int(value) = level.get::<_, &NbtTag>("xPos").unwrap() {
                Some(*value)
            } else {
                None
            }
            .unwrap();
            let chunk_z = if let NbtTag::Int(value) = level.get::<_, &NbtTag>("zPos").unwrap() {
                Some(*value)
            } else {
                None
            }
            .unwrap();

            let header_offset = ((chunk_x & 0x1f) + ((chunk_z & 0x1f) << 5)) as usize;

            // read the existing header data
            let existing_packed = read_bigendian_u32(bytes, header_offset * 4);
            let existing_offset = existing_packed >> SIZE_BITS;
            let existing_size = (existing_packed & SIZE_MASK) as u8;

            // this is the current entry if the header points to this entry
            let is_current_entry =
                existing_offset == sector_idx as u32 && existing_size == size_sectors;

            let entry = RegionEntry {
                offset_sectors: sector_idx as u32,
                size_sectors,
                is_current: is_current_entry,
            };

            // compute if absent
            let existing_entries = match discovered_entries.get_mut(header_offset) {
                None => {
                    discovered_entries.insert(header_offset, Vec::new());
                    &mut discovered_entries[header_offset]
                }
                Some(existing) => existing,
            };
            existing_entries.push(entry);
        }
    }

    discovered_entries
}

fn recover_entries(
    file_path: &Path,
    duplicate_behaviour: Option<DuplicateBehaviour>,
) -> Result<(), Error> {
    let mut bytes = fs::read(file_path)?;

    let file_name = file_path.file_name().unwrap().to_str().unwrap().to_owned();
    let file_name_split: Vec<&str> = file_name.split('.').collect(); // filenames look like: r.X.Z.mca
    let region_position: (i32, i32) = (
        file_name_split[1].parse().unwrap(),
        file_name_split[2].parse().unwrap(),
    );

    let entries_by_header_idx = discover_all_entries(&bytes);

    let mut any_recovered = false;

    for (header_idx, entries) in entries_by_header_idx.iter().enumerate() {
        if entries.is_empty() {
            continue;
        }

        let entry_to_save;
        if entries.len() == 1 {
            //there is only one option, so take it
            entry_to_save = &entries[0];
            if entry_to_save.is_current {
                continue; // entry is the current active entry, so we can skip it
            }
        } else {
            let mut current_count: u32 = 0;
            for entry in entries {
                if entry.is_current {
                    current_count += 1;
                }
            }
            assert!(current_count <= 1);
            let has_current = current_count == 1;

            let mut untracked_count: u32 = 0;
            for entry in entries {
                if !entry.is_current {
                    untracked_count += 1;
                }
            }
            let has_untracked = untracked_count > 0;
            let has_multiple_untracked = untracked_count > 1;

            let (chunk_x, chunk_z) =
                chunk_position_from_entry_idx(region_position, header_idx as u16);

            let current_behaviour = match duplicate_behaviour {
                None => {
                    if has_current && has_untracked {
                        println!(
                            "Chunk ({}, {}) has {} known entries and {} unknown entries",
                            chunk_x, chunk_z, current_count, untracked_count
                        );
                        ask_for_duplicate_behaviour()
                    } else if has_current {
                        TakeCurrent
                    } else if has_untracked {
                        TakeUntracked
                    } else {
                        panic!("Entry is neither current nor untracked?!");
                    }
                }
                Some(behaviour) => behaviour,
            };

            if has_current && current_behaviour == TakeCurrent {
                continue; // user wants to keep the current entry, so skip it
            } else if untracked_count == 1 && current_behaviour == TakeUntracked {
                // there is only one untracked, so save it
                entry_to_save = entries.iter().find(|entry| !entry.is_current).unwrap();
                println!("Chunk ({}, {}) recovered unknown entry!", chunk_x, chunk_z);
            } else if has_multiple_untracked {
                // there are multiple untracked, so allow user to pick from them
                let mut untracked_entries = Vec::new();
                for entry in entries {
                    if !entry.is_current {
                        untracked_entries.push(entry);
                    }
                }

                println!(
                    "Which unknown entry should be chosen (1 to {})?",
                    untracked_entries.len()
                );
                let entry_idx = ask_for_integer(|value| value > 0) - 1;
                entry_to_save = untracked_entries[entry_idx as usize];
                println!("Chunk ({}, {}) recovered unknown entry!", chunk_x, chunk_z);
            } else {
                panic!("Should never be reached");
            }
        }

        any_recovered = true;
        set_header_entry(
            &mut bytes,
            header_idx * 4,
            entry_to_save.offset_sectors as usize,
            entry_to_save.size_sectors as u8,
        );
    }

    if any_recovered {
        fs::write(file_path, bytes)?;
        println!(
            "Wrote to region r.{}.{}.mca",
            region_position.0, region_position.1
        );
    }

    Ok(())
}

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    #[clap(long, short)]
    pub world_path: PathBuf,

    #[clap(long, short)]
    pub duplicate_behaviour: Option<DuplicateBehaviour>,
}

fn main() -> std::io::Result<()> {
    let args = Args::parse();

    let world_path = args.world_path.join("region");

    let world_path = Path::new(&world_path);
    for entry in fs::read_dir(world_path)? {
        let entry = entry?;
        let entry_path = entry.path();
        if !entry_path.is_dir() {
            let extension = entry_path.extension();
            if let Some(ext) = extension {
                if ext.to_str().unwrap().ends_with("mca") {
                    match recover_entries(&entry.path(), args.duplicate_behaviour) {
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
