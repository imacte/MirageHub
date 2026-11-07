use super::{ChromaError, Result};
use crate::effects::frame::Colour;
use serde_json::Value;

pub type Grid = Vec<Vec<Colour>>;
pub const GRIDS: &[(&str, [usize; 2])] = &[
    ("keyboard", [6, 22]),
    ("mouse", [9, 7]),
    ("headset", [1, 5]),
    ("mousepad", [1, 15]),
    ("keypad", [4, 5]),
    ("chromalink", [1, 5]),
];
pub const BLACK: Colour = [0, 0, 0];
const MAX_SIDE: usize = 32;
const KEY_FLAG: u32 = 0x01000000;

fn number(value: &Value) -> Result<u32> {
    value
        .as_u64()
        .map(|v| v as u32)
        .or_else(|| {
            value
                .as_str()
                .and_then(|s| s.trim().parse::<i128>().ok())
                .map(|v| v as u32)
        })
        .or_else(|| crate::device::integer(value).ok().map(|v| v as u32))
        .ok_or_else(|| ChromaError::Invalid(format!("a colour is a number: {value}")))
}
pub fn colour_of(value: &Value) -> Result<Colour> {
    Ok(colour(number(value)?))
}
fn colour(value: u32) -> Colour {
    [value as u8, (value >> 8) as u8, (value >> 16) as u8]
}
pub fn dimensions(category: &str) -> Option<[usize; 2]> {
    GRIDS
        .iter()
        .find(|(name, _)| *name == category)
        .map(|(_, dims)| *dims)
}

fn rows(param: &Value, what: &str) -> Result<Grid> {
    let array = param
        .as_array()
        .filter(|a| !a.is_empty() && a.len() <= MAX_SIDE)
        .ok_or_else(|| ChromaError::Invalid(format!("{what} is a list of rows")))?;
    let rows: Vec<&[Value]> = if array.iter().all(Value::is_array) {
        array
            .iter()
            .map(|v| v.as_array().unwrap().as_slice())
            .collect()
    } else {
        vec![array]
    };
    if rows.iter().any(|row| row.len() > MAX_SIDE) {
        return Err(ChromaError::Invalid(format!(
            "{what} is wider than any device"
        )));
    }
    rows.into_iter()
        .map(|row| row.iter().map(colour_of).collect())
        .collect()
}
fn truth(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(v) => *v,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(v) => !v.is_empty(),
        Value::Array(v) => !v.is_empty(),
        Value::Object(v) => !v.is_empty(),
    }
}
fn keys_over(mut grid: Grid, keys: &Value) -> Result<Grid> {
    if !truth(keys) {
        return Ok(grid);
    }
    let array = keys
        .as_array()
        .filter(|a| a.len() <= MAX_SIDE)
        .ok_or_else(|| ChromaError::Invalid("\"key\" is a list of rows".into()))?;
    let rows: Vec<&[Value]> = if array.iter().all(Value::is_array) {
        array
            .iter()
            .map(|v| v.as_array().unwrap().as_slice())
            .collect()
    } else {
        vec![array]
    };
    // Validate all supplied rows, even those outside the colour grid.
    let rows: Vec<Vec<u32>> = rows
        .into_iter()
        .map(|r| r.iter().take(MAX_SIDE).map(number).collect())
        .collect::<Result<_>>()?;
    for (row, keys) in grid.iter_mut().zip(rows) {
        for (pixel, value) in row.iter_mut().zip(keys) {
            if value & KEY_FLAG != 0 {
                *pixel = colour(value);
            }
        }
    }
    Ok(grid)
}
pub fn parse(category: &str, body: &Value) -> Result<Grid> {
    let [height, width] = dimensions(category)
        .ok_or_else(|| ChromaError::Invalid(format!("no such kind of device: {category}")))?;
    let body = body.as_object().ok_or_else(|| {
        ChromaError::Invalid("an effect is an object with \"effect\" and \"param\"".into())
    })?;
    let effect = body.get("effect").and_then(Value::as_str).unwrap_or("");
    let param = body.get("param").unwrap_or(&Value::Null);
    match effect {
        "CHROMA_NONE" => Ok(vec![vec![BLACK; width]; height]),
        "CHROMA_STATIC" => {
            let value = if param.is_object() {
                &param["color"]
            } else {
                param
            };
            Ok(vec![vec![colour_of(value)?; width]; height])
        }
        "CHROMA_CUSTOM" => rows(param, "CHROMA_CUSTOM"),
        "CHROMA_CUSTOM_KEY" if category == "keyboard" => {
            if !param.is_object() {
                return Err(ChromaError::Invalid(
                    "CHROMA_CUSTOM_KEY takes {\"color\": rows, \"key\": rows}".into(),
                ));
            }
            keys_over(rows(&param["color"], "color")?, &param["key"])
        }
        "CHROMA_CUSTOM2" if category == "keyboard" => {
            if !param.is_object() {
                return Err(ChromaError::Invalid(
                    "CHROMA_CUSTOM2 takes {\"color\": rows, \"key\": rows}".into(),
                ));
            }
            let extended = rows(&param["color"], "color")?;
            let inner = extended
                .iter()
                .skip(1)
                .take(height)
                .map(|row| {
                    (1..=width)
                        .map(|col| row.get(col).copied().unwrap_or(BLACK))
                        .collect()
                })
                .collect();
            keys_over(inner, &param["key"])
        }
        "CHROMA_CUSTOM2" => rows(param, "CHROMA_CUSTOM2"),
        _ => Err(ChromaError::Invalid(format!(
            "{effect} cannot be drawn on a {category}"
        ))),
    }
}
pub fn at(grid: &Grid, row: usize, column: usize) -> Colour {
    grid.get(row)
        .and_then(|r| r.get(column))
        .copied()
        .unwrap_or(BLACK)
}
pub fn average(colours: &[Colour]) -> Colour {
    if colours.is_empty() {
        return BLACK;
    }
    std::array::from_fn(|channel| {
        (colours
            .iter()
            .map(|c| usize::from(c[channel]))
            .sum::<usize>()
            / colours.len()) as u8
    })
}
pub fn strip(grid: &Grid) -> Vec<Colour> {
    grid.iter().flatten().copied().collect()
}
pub fn resample(colours: &[Colour], count: usize) -> Vec<Colour> {
    if colours.is_empty() {
        return vec![BLACK; count];
    }
    if count == 1 {
        return vec![colours[0]];
    }
    (0..count)
        .map(|index| {
            // Python's round chooses the even neighbour on a half-way sample.
            let numerator = index * (colours.len() - 1);
            let denominator = count - 1;
            let (whole, remainder) = (numerator / denominator, numerator % denominator);
            let rounded = whole
                + usize::from(
                    remainder * 2 > denominator || (remainder * 2 == denominator && whole % 2 == 1),
                );
            colours[rounded]
        })
        .collect()
}
pub fn mouse_zone(grid: &Grid, zone: &str) -> Colour {
    match zone {
        "scroll" => at(grid, 2, 3),
        "logo" => at(grid, 7, 3),
        "left" | "right" => average(
            &(1..8)
                .map(|r| at(grid, r, if zone == "left" { 0 } else { 6 }))
                .collect::<Vec<_>>(),
        ),
        _ => at(grid, 4, 3),
    }
}
pub fn mouse_strip(grid: &Grid, count: usize) -> Vec<Colour> {
    if count == 1 {
        return vec![at(grid, 7, 3)];
    }
    let sides: Vec<_> = [0, 6]
        .into_iter()
        .flat_map(|col| (1..8).map(move |row| at(grid, row, col)))
        .collect();
    resample(&sides, count)
}
