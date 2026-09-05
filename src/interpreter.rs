//! A permissive, top-level-only reader for the Unreal "tagged property" list found inside
//! the decompressed save data. It does not attempt to understand every custom struct schema
//! Bethesda/Virtuos defined (that would need per-type "hints" nobody has published yet) — it
//! just walks the property list using each property's own declared byte length to jump to the
//! next one, decoding simple scalar values (numbers, strings, bools, names) directly and
//! summarizing anything more complex (structs, arrays, maps) by name, type and size. That's
//! already enough to spot quest-, item- and stat-shaped property names in the output.

use byteorder::{LittleEndian, ReadBytesExt};
use gvas::cursor_ext::ReadExt;
use gvas::GvasHeader;
use std::io::{Cursor, Read, Seek, SeekFrom};

pub struct InterpretResult {
    pub text: String,
    pub properties_found: usize,
    pub stopped_early: bool,
}

fn read_type_specific_header(cursor: &mut Cursor<&[u8]>, ptype: &str) -> std::io::Result<String> {
    Ok(match ptype {
        "StructProperty" => {
            let struct_type = read_gvas_string(cursor)?;
            let mut guid = [0u8; 16];
            cursor.read_exact(&mut guid)?;
            struct_type
        }
        "ArrayProperty" | "SetProperty" => read_gvas_string(cursor)?,
        "MapProperty" => {
            let key_type = read_gvas_string(cursor)?;
            let value_type = read_gvas_string(cursor)?;
            format!("{key_type} -> {value_type}")
        }
        "EnumProperty" | "ByteProperty" => read_gvas_string(cursor)?,
        _ => String::new(),
    })
}

fn read_gvas_string(cursor: &mut Cursor<&[u8]>) -> std::io::Result<String> {
    ReadExt::read_string(cursor)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{e:?}")))
}

fn describe_value(cursor: &mut Cursor<&[u8]>, ptype: &str, extra: &str, length: u32) -> String {
    let value_start = cursor.position();
    let result: std::io::Result<String> = (|| {
        Ok(match ptype {
            "IntProperty" => cursor.read_i32::<LittleEndian>()?.to_string(),
            "UInt32Property" => cursor.read_u32::<LittleEndian>()?.to_string(),
            "Int64Property" => cursor.read_i64::<LittleEndian>()?.to_string(),
            "UInt64Property" => cursor.read_u64::<LittleEndian>()?.to_string(),
            "Int16Property" => cursor.read_i16::<LittleEndian>()?.to_string(),
            "Int8Property" => cursor.read_i8()?.to_string(),
            "FloatProperty" => cursor.read_f32::<LittleEndian>()?.to_string(),
            "DoubleProperty" => cursor.read_f64::<LittleEndian>()?.to_string(),
            "StrProperty" => ReadExt::read_fstring(cursor)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{e:?}")))?
                .unwrap_or_default(),
            "NameProperty" => read_gvas_string(cursor)?,
            "ByteProperty" if extra == "None" => cursor.read_u8()?.to_string(),
            "ByteProperty" => read_gvas_string(cursor)?,
            _ => format!("<{length} bytes>"),
        })
    })();
    // Always snap back to the declared end, regardless of what we managed to read — this is
    // what makes the walk resilient to struct/array contents we don't understand.
    let _ = cursor.seek(SeekFrom::Start(value_start + length as u64));
    result.unwrap_or_else(|_| format!("<{length} bytes, unreadable>"))
}

/// Walks the top-level Unreal property list inside `data` (which should start with, or start
/// 4 bytes before, the `GVAS` magic) and produces a readable summary: one line per top-level
/// property with its name, type, and either a decoded scalar value or a size/type summary for
/// anything nested (structs, arrays, maps — Bethesda's custom item/quest/actor data lives in
/// those, but decoding their internal layout is a separate, larger effort — see the README).
pub fn interpret(data: &[u8]) -> InterpretResult {
    let mut offset = 0usize;
    if data.len() > 8 && &data[4..8] == b"GVAS" {
        offset = 4;
    }
    let slice = &data[offset..];
    let mut cursor = Cursor::new(slice);

    let mut text = String::new();
    let mut properties_found = 0usize;
    let mut stopped_early = false;

    match GvasHeader::read(&mut cursor) {
        Ok(header) => {
            let _ = &header;
        }
        Err(e) => {
            return InterpretResult {
                text: format!("Could not read the inner save header: {e:?}\n"),
                properties_found: 0,
                stopped_early: true,
            };
        }
    }

    loop {
        let name = match read_gvas_string(&mut cursor) {
            Ok(n) => n,
            Err(_) => {
                stopped_early = true;
                break;
            }
        };
        if name == "None" {
            break;
        }

        let ptype = match read_gvas_string(&mut cursor) {
            Ok(t) => t,
            Err(_) => {
                stopped_early = true;
                break;
            }
        };

        if ptype == "BoolProperty" {
            match (|| -> std::io::Result<bool> {
                let size = cursor.read_u64::<LittleEndian>()?;
                if size != 0 {
                    return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "bad bool size"));
                }
                let value = cursor.read_u8()? != 0;
                let _indicator = cursor.read_u8()?;
                Ok(value)
            })() {
                Ok(value) => {
                    let _ = writeln!(&mut text, "{name}: BoolProperty = {value}");
                    properties_found += 1;
                    continue;
                }
                Err(_) => {
                    stopped_early = true;
                    break;
                }
            }
        }

        let length = match cursor.read_u32::<LittleEndian>() {
            Ok(v) => v,
            Err(_) => {
                stopped_early = true;
                break;
            }
        };
        let array_index = match cursor.read_u32::<LittleEndian>() {
            Ok(v) => v,
            Err(_) => {
                stopped_early = true;
                break;
            }
        };

        let extra = match read_type_specific_header(&mut cursor, &ptype) {
            Ok(e) => e,
            Err(_) => {
                stopped_early = true;
                break;
            }
        };

        let terminator = match cursor.read_u8() {
            Ok(v) => v,
            Err(_) => {
                stopped_early = true;
                break;
            }
        };
        if terminator != 0 {
            stopped_early = true;
            break;
        }

        let value_str = describe_value(&mut cursor, &ptype, &extra, length);
        properties_found += 1;

        let idx_note = if array_index != 0 { format!(" [{array_index}]") } else { String::new() };
        if extra.is_empty() {
            let _ = writeln!(&mut text, "{name}{idx_note}: {ptype} = {value_str}");
        } else {
            let _ = writeln!(&mut text, "{name}{idx_note}: {ptype}<{extra}> = {value_str}");
        }

        if properties_found > 200_000 {
            let _ = writeln!(&mut text, "\n[stopped after 200,000 properties as a safety limit]");
            break;
        }
    }

    if stopped_early {
        let _ = writeln!(
            &mut text,
            "\n[stopped after {properties_found} properties — either the end of the readable \
             list was reached, or a property type this tool doesn't know how to skip over yet \
             was hit. Everything above this line is reliable.]"
        );
    }

    InterpretResult { text, properties_found, stopped_early }
}

use std::fmt::Write as _;
