//! A permissive reader for the Unreal "tagged property" lists found inside the decompressed
//! save data. It does not attempt to understand every custom struct schema Bethesda/Virtuos
//! defined (that would need per-type "hints" nobody has published yet) — it walks each
//! property list using every property's own declared byte length to jump to the next one,
//! decoding simple scalar values (numbers, strings, bools, names) directly, and *recursing*
//! into structs and struct arrays (which is where the interesting content lives) since their
//! internal layout is just another tagged-property list in Unreal's format. Anything it can't
//! confidently walk (an unknown array/map element framing) is reported by name/type/size
//! instead of guessed at, so the output never shows corrupted-looking data.

use byteorder::{LittleEndian, ReadBytesExt};
use gvas::cursor_ext::ReadExt;
use gvas::GvasHeader;
use std::fmt::Write as _;
use std::io::{Cursor, Read, Seek, SeekFrom};

pub struct InterpretResult {
    pub text: String,
    pub properties_found: usize,
    pub stopped_early: bool,
}

const MAX_DEPTH: usize = 12;
const MAX_PROPERTIES: usize = 200_000;

struct Walker {
    text: String,
    properties_found: usize,
}

fn read_gvas_string(cursor: &mut Cursor<&[u8]>) -> std::io::Result<String> {
    ReadExt::read_string(cursor)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{e:?}")))
}

fn indent(depth: usize) -> String {
    "  ".repeat(depth)
}

/// Reads one property's type-specific extra header fields (after length+array_index, before
/// the terminator byte). Returns a short label for display (struct type name, inner type, etc).
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

/// Tries to read a nested property list (a struct's own fields, terminated by a "None" name)
/// occupying exactly `length` bytes starting at the cursor's current position. Returns Some on
/// a clean parse (used the exact byte range, ended on "None"), None if anything looks off —
/// callers fall back to a plain size summary rather than show a possibly-garbled partial walk.
fn try_walk_as_property_list(
    cursor: &mut Cursor<&[u8]>,
    length: u32,
    depth: usize,
    w: &mut Walker,
) -> Option<()> {
    if depth >= MAX_DEPTH {
        return None;
    }
    let start = cursor.position();
    let end = start + length as u64;
    let mut local = String::new();
    let mut local_count = 0usize;

    loop {
        if cursor.position() >= end {
            #[cfg(feature = "trace-interp")]
            eprintln!("TRACE: ran past end without None, pos={} end={}", cursor.position(), end);
            return None; // ran past the end without hitting "None" — not a clean property list
        }
        let name = match read_gvas_string(cursor) {
            Ok(n) => n,
            Err(_e) => {
                #[cfg(feature = "trace-interp")]
                eprintln!("TRACE: failed reading name at pos={}: {:?}", cursor.position(), _e);
                return None;
            }
        };
        if name == "None" {
            break;
        }
        local_count += 1;
        if w.properties_found + local_count > MAX_PROPERTIES {
            return None;
        }
        #[cfg(feature = "trace-interp")]
        eprintln!("TRACE: field '{}' at pos={}", name, cursor.position());
        if let Err(_e) = walk_one_property(cursor, &name, depth, &mut local) {
            #[cfg(feature = "trace-interp")]
            eprintln!("TRACE: failed walking property '{}' at pos={}: {:?}", name, cursor.position(), _e);
            return None;
        }
    }

    // Some structs (their name/type is the giveaway — "Serialized..." ones especially) use
    // Unreal's reflection system for a handful of UPROPERTY fields, terminated by the normal
    // "None", and then append additional hand-serialized raw bytes afterward within the same
    // declared length — a native C++ `Serialize()` override tacking custom data onto the
    // standard property list. That's not a parse failure, just extra unstructured data.
    let trailing = end.saturating_sub(cursor.position());
    if trailing > 0 {
        let _ = writeln!(&mut local, "{}[+ {trailing} more bytes of non-property data]", indent(depth));
    }

    w.text.push_str(&local);
    w.properties_found += local_count;
    Some(())
}

/// Tries to read `ArrayProperty<StructProperty>`-shaped data. Empirically (traced byte-by-byte
/// against a real save), this game's struct arrays are NOT a flat list of N identically-tagged
/// elements — after a leading 4-byte field (nominally a "count", not validated as such here),
/// there is exactly one fully self-tagged struct (its own name/type/length/struct-type/guid
/// header, same shape as any other StructProperty) whose declared length exactly accounts for
/// the rest of the array's byte budget. So: skip 4 bytes, then walk exactly one nested
/// property the normal way, and require that it lands exactly on the array's own end. Bails
/// out cleanly (returns None) if that doesn't hold, rather than risk printing corrupted output.
fn try_walk_struct_array(
    cursor: &mut Cursor<&[u8]>,
    length: u32,
    depth: usize,
    w: &mut Walker,
) -> Option<()> {
    if depth >= MAX_DEPTH || length < 4 {
        return None;
    }
    let start = cursor.position();
    let end = start + length as u64;
    cursor.seek(SeekFrom::Start(start + 4)).ok()?; // skip the leading 4-byte field

    let mut local = String::new();
    let name = read_gvas_string(cursor).ok()?;
    if name == "None" {
        return None;
    }
    walk_one_property(cursor, &name, depth, &mut local).ok()?;

    if cursor.position() != end {
        return None;
    }

    w.properties_found += 1;
    w.text.push_str(&local);
    Some(())
}

fn describe_scalar(cursor: &mut Cursor<&[u8]>, ptype: &str, extra: &str) -> std::io::Result<String> {
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
        _ => return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "not scalar")),
    })
}

/// Reads and appends one full property (name already consumed by the caller) to `out`.
fn walk_one_property(
    cursor: &mut Cursor<&[u8]>,
    name: &str,
    depth: usize,
    out_owner: &mut String,
) -> std::io::Result<()> {
    let ptype = read_gvas_string(cursor)?;
    let ind = indent(depth);

    if ptype == "BoolProperty" {
        let size = cursor.read_u64::<LittleEndian>()?;
        if size != 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "bad bool size"));
        }
        let value = cursor.read_u8()? != 0;
        let _indicator = cursor.read_u8()?;
        let _ = writeln!(out_owner, "{ind}{name}: BoolProperty = {value}");
        return Ok(());
    }

    let length = cursor.read_u32::<LittleEndian>()?;
    let array_index = cursor.read_u32::<LittleEndian>()?;
    #[cfg(feature = "trace-interp")]
    eprintln!("TRACE:   '{name}' ptype={ptype} length={length} array_index={array_index} pos={}", cursor.position());
    let extra = read_type_specific_header(cursor, &ptype)?;
    let terminator = cursor.read_u8()?;
    if terminator != 0 {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "bad terminator"));
    }

    let value_start = cursor.position();
    let idx_note = if array_index != 0 { format!(" [{array_index}]") } else { String::new() };
    let type_label = if extra.is_empty() { ptype.clone() } else { format!("{ptype}<{extra}>") };

    // Try to recurse for the types that hold the interesting nested data. Every attempt is
    // bounded to exactly `length` bytes and snapped back afterward, so a failed/partial
    // attempt can never desync the rest of the walk.
    let recursed = match ptype.as_str() {
        // Blittable structs with no internal tagged-property list — decode their raw bytes
        // directly instead of (harmlessly, but pointlessly) trying and failing to recurse.
        "StructProperty" if extra == "Timespan" => {
            let ticks = cursor.read_i64::<LittleEndian>().unwrap_or(0);
            cursor.seek(SeekFrom::Start(value_start + length as u64))?;
            let seconds = ticks as f64 / 10_000_000.0;
            let hours = seconds / 3600.0;
            let _ = writeln!(out_owner, "{ind}{name}{idx_note}: {type_label} = {hours:.2} hours ({seconds:.0}s)");
            true
        }
        "StructProperty" if extra == "DateTime" => {
            // .NET/Unreal FDateTime ticks are 100ns units since 0001-01-01; Unix epoch sits at
            // tick 621355968000000000. Convert so this is at least a recognizable timestamp.
            let ticks = cursor.read_i64::<LittleEndian>().unwrap_or(0);
            cursor.seek(SeekFrom::Start(value_start + length as u64))?;
            const UNIX_EPOCH_TICKS: i64 = 621_355_968_000_000_000;
            let unix_seconds = (ticks - UNIX_EPOCH_TICKS) / 10_000_000;
            let _ = writeln!(out_owner, "{ind}{name}{idx_note}: {type_label} = unix {unix_seconds}");
            true
        }
        "StructProperty" if extra == "Guid" => {
            let mut bytes = [0u8; 16];
            let ok = cursor.read_exact(&mut bytes).is_ok();
            cursor.seek(SeekFrom::Start(value_start + length as u64))?;
            if ok {
                let hex: String = bytes.iter().map(|b| format!("{b:02X}")).collect();
                let _ = writeln!(out_owner, "{ind}{name}{idx_note}: {type_label} = {hex}");
            }
            ok
        }
        "StructProperty" => {
            let mut w = Walker { text: String::new(), properties_found: 0 };
            let ok = try_walk_as_property_list(cursor, length, depth + 1, &mut w).is_some();
            cursor.seek(SeekFrom::Start(value_start + length as u64))?;
            if ok {
                let _ = writeln!(out_owner, "{ind}{name}{idx_note}: {type_label} {{");
                out_owner.push_str(&w.text);
                let _ = writeln!(out_owner, "{ind}}}");
                true
            } else {
                false
            }
        }
        "ArrayProperty" if extra == "StructProperty" => {
            let mut w = Walker { text: String::new(), properties_found: 0 };
            let ok = try_walk_struct_array(cursor, length, depth + 1, &mut w).is_some();
            cursor.seek(SeekFrom::Start(value_start + length as u64))?;
            if ok {
                let _ = writeln!(out_owner, "{ind}{name}{idx_note}: {type_label} [");
                out_owner.push_str(&w.text);
                let _ = writeln!(out_owner, "{ind}]");
                true
            } else {
                false
            }
        }
        _ => false,
    };

    if !recursed {
        let value_str = match describe_scalar(cursor, &ptype, &extra) {
            Ok(v) => v,
            Err(_) => format!("<{length} bytes>"),
        };
        cursor.seek(SeekFrom::Start(value_start + length as u64))?;
        let _ = writeln!(out_owner, "{ind}{name}{idx_note}: {type_label} = {value_str}");
    }

    Ok(())
}

/// Walks the top-level Unreal property list inside `data` (which should start with, or start
/// 4 bytes before, the `GVAS` magic) and produces a readable, indented summary. Structs and
/// struct-arrays are expanded recursively wherever the byte-length bookkeeping checks out
/// cleanly; anything that doesn't is reported by name/type/size instead of guessed at.
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

    if let Err(e) = GvasHeader::read(&mut cursor) {
        return InterpretResult {
            text: format!("Could not read the inner save header: {e:?}\n"),
            properties_found: 0,
            stopped_early: true,
        };
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

        match walk_one_property(&mut cursor, &name, 0, &mut text) {
            Ok(()) => properties_found += 1,
            Err(_) => {
                stopped_early = true;
                break;
            }
        }

        if properties_found > MAX_PROPERTIES {
            let _ = writeln!(&mut text, "\n[stopped after {MAX_PROPERTIES} properties as a safety limit]");
            break;
        }
    }

    if stopped_early {
        let _ = writeln!(
            &mut text,
            "\n[stopped after {properties_found} top-level properties — either the end of the \
             readable list was reached, or a property type this tool doesn't know how to skip \
             over yet was hit. Everything above this line is reliable.]"
        );
    }

    InterpretResult { text, properties_found, stopped_early }
}
