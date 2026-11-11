//! A reader for the YAML that Unity writes.
//!
//! Unity's text assets are a narrow, regular subset of YAML: block maps, block
//! sequences whose dashes sit at their key's indent, one-line flow maps such as
//! `{x: 0, y: 1}`, and plain scalars. This reads exactly that and nothing more,
//! which keeps the converter free of a YAML dependency that would mostly be
//! spent rejecting Unity's `!u!` tags.

use std::fmt;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Scalar(String),
    Seq(Vec<Value>),
    /// In file order. Unity never repeats a key within a map.
    Map(Vec<(String, Value)>),
}

static EMPTY: Value = Value::Scalar(String::new());

impl Value {
    /// The value under `key`, or an empty scalar when this is not a map or has no
    /// such key. Lookups chain without checks: `doc.get("a").get("b").f32()`.
    pub fn get(&self, key: &str) -> &Value {
        match self {
            Value::Map(entries) => entries
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value)
                .unwrap_or(&EMPTY),
            _ => &EMPTY,
        }
    }

    pub fn has(&self, key: &str) -> bool {
        matches!(self, Value::Map(entries) if entries.iter().any(|(name, _)| name == key))
    }

    pub fn str(&self) -> &str {
        match self {
            Value::Scalar(text) => text,
            _ => "",
        }
    }

    pub fn f32(&self) -> f32 {
        self.str().parse().unwrap_or(0.0)
    }

    pub fn i64(&self) -> i64 {
        self.str().parse().unwrap_or(0)
    }

    /// Unity writes booleans as 0 and 1.
    pub fn bool(&self) -> bool {
        self.i64() != 0
    }

    pub fn seq(&self) -> &[Value] {
        match self {
            Value::Seq(items) => items,
            _ => &[],
        }
    }

    /// Sets the value at a Unity property path such as `m_Color.r` or
    /// `m_Layers.Array.data[2]`, creating maps and growing sequences as needed.
    /// This is how a prefab instance's overrides are applied to its source.
    pub fn set_path(&mut self, path: &str, value: &str) {
        let (head, rest) = match path.split_once('.') {
            Some((head, rest)) => (head, Some(rest)),
            None => (path, None),
        };

        // `Array.size` and `Array.data[i]` address this value as a sequence.
        if head == "Array" {
            let Some(rest) = rest else {
                return;
            };
            if !matches!(self, Value::Seq(_)) {
                *self = Value::Seq(Vec::new());
            }
            let Value::Seq(items) = self else {
                return;
            };
            if rest == "size" {
                items.resize(value.parse().unwrap_or(items.len()), EMPTY.clone());
                return;
            }
            let (element, deeper) = match rest.split_once('.') {
                Some((element, deeper)) => (element, Some(deeper)),
                None => (rest, None),
            };
            let index = element
                .strip_prefix("data[")
                .and_then(|text| text.strip_suffix(']'))
                .and_then(|text| text.parse::<usize>().ok());
            let Some(index) = index else {
                return;
            };
            if items.len() <= index {
                items.resize(index + 1, EMPTY.clone());
            }
            match deeper {
                Some(deeper) => items[index].set_path(deeper, value),
                None => items[index] = Value::Scalar(value.to_string()),
            }
            return;
        }

        if !matches!(self, Value::Map(_)) {
            *self = Value::Map(Vec::new());
        }
        let Value::Map(entries) = self else {
            return;
        };
        let position = match entries.iter().position(|(name, _)| name == head) {
            Some(position) => position,
            None => {
                entries.push((head.to_string(), EMPTY.clone()));
                entries.len() - 1
            }
        };
        match rest {
            Some(rest) => entries[position].1.set_path(rest, value),
            None => entries[position].1 = Value::Scalar(value.to_string()),
        }
    }

    pub fn entries(&self) -> &[(String, Value)] {
        match self {
            Value::Map(entries) => entries,
            _ => &[],
        }
    }
}

/// One `--- !u!<class> &<id>` section of a Unity file.
#[derive(Clone, Debug)]
pub struct Document {
    /// The object's id within its file: what `{fileID: ...}` references point at.
    pub file_id: i64,
    /// A placeholder for an object that lives in a nested prefab.
    pub stripped: bool,
    /// The class name, e.g. `Tilemap`.
    pub class: String,
    /// The object's fields.
    pub body: Value,
}

#[derive(Debug)]
pub struct ParseError {
    pub line: usize,
    pub message: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for ParseError {}

/// Splits a scene or prefab file into its objects.
pub fn parse_documents(text: &str) -> Result<Vec<Document>, ParseError> {
    let lines: Vec<&str> = text.lines().collect();
    let mut documents = Vec::new();
    let mut index = 0;

    while index < lines.len() {
        let Some(header) = lines[index].strip_prefix("--- !u!") else {
            // The `%YAML` and `%TAG` preamble.
            index += 1;
            continue;
        };
        let mut parts = header.split_whitespace();
        // The numeric class id comes first; the class name on the next line says the same.
        let file_id = parts
            .nth(1)
            .and_then(|part| part.strip_prefix('&'))
            .and_then(|part| part.parse().ok());
        let Some(file_id) = file_id else {
            return Err(ParseError {
                line: index + 1,
                message: format!("unreadable object header `{}`", lines[index]),
            });
        };
        let stripped = parts.next() == Some("stripped");

        let start = index + 1;
        let mut end = start;
        while end < lines.len() && !lines[end].starts_with("--- ") {
            end += 1;
        }

        let body = parse_lines(&lines[start..end], start)?;
        let (class, body) = match body {
            Value::Map(mut entries) if entries.len() == 1 => entries.remove(0),
            _ => {
                return Err(ParseError {
                    line: start + 1,
                    message: "an object should be one class name with its fields".to_string(),
                });
            }
        };
        documents.push(Document {
            file_id,
            stripped,
            class,
            body,
        });
        index = end;
    }

    Ok(documents)
}

/// Parses a file that is one plain map, such as a `.meta` file.
pub fn parse_map(text: &str) -> Result<Value, ParseError> {
    let lines: Vec<&str> = text.lines().filter(|line| !line.starts_with('%')).collect();
    parse_lines(&lines, 0)
}

fn parse_lines(lines: &[&str], first_line: usize) -> Result<Value, ParseError> {
    let mut parser = Parser {
        lines,
        first_line,
        position: 0,
    };
    parser.skip_blank();
    if parser.position >= lines.len() {
        return Ok(Value::Map(Vec::new()));
    }
    let indent = indent_of(lines[parser.position]);
    parser.block(indent)
}

struct Parser<'a> {
    lines: &'a [&'a str],
    first_line: usize,
    position: usize,
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

impl Parser<'_> {
    fn skip_blank(&mut self) {
        while self.position < self.lines.len() && self.lines[self.position].trim().is_empty() {
            self.position += 1;
        }
    }

    /// The indent of the next non-blank line, if any.
    fn peek_indent(&mut self) -> Option<usize> {
        self.skip_blank();
        self.lines.get(self.position).map(|line| indent_of(line))
    }

    fn error(&self, message: impl Into<String>) -> ParseError {
        ParseError {
            line: self.first_line + self.position + 1,
            message: message.into(),
        }
    }

    /// A map or a sequence whose lines start at `indent`.
    fn block(&mut self, indent: usize) -> Result<Value, ParseError> {
        self.skip_blank();
        let line = self.lines[self.position];
        if is_dash(&line[indent_of(line)..]) {
            self.sequence(indent)
        } else {
            self.map(indent)
        }
    }

    fn map(&mut self, indent: usize) -> Result<Value, ParseError> {
        let mut entries = Vec::new();
        while let Some(found) = self.peek_indent() {
            if found != indent {
                break;
            }
            let content = &self.lines[self.position][indent..];
            if is_dash(content) {
                break;
            }
            let (key, rest) = split_key(content).ok_or_else(|| self.error("expected `key: value`"))?;
            self.position += 1;
            let value = self.value_after_key(indent, rest)?;
            entries.push((key.to_string(), value));
        }
        Ok(Value::Map(entries))
    }

    /// The value of a key whose own line had `rest` after the colon.
    fn value_after_key(&mut self, key_indent: usize, rest: &str) -> Result<Value, ParseError> {
        if !rest.is_empty() {
            let mut text = rest.to_string();
            // A long scalar continues on deeper lines that are not keys of their own.
            while let Some(found) = self.peek_indent() {
                let line = self.lines[self.position];
                if found <= key_indent || !continues_scalar(&text, line.trim()) {
                    break;
                }
                text.push(' ');
                text.push_str(line.trim());
                self.position += 1;
            }
            return self.inline(&text);
        }

        match self.peek_indent() {
            Some(found) if found > key_indent => self.block(found),
            // Unity puts a sequence's dashes at the same indent as its key.
            Some(found) if found == key_indent && is_dash(&self.lines[self.position][found..]) => {
                self.sequence(found)
            }
            _ => Ok(Value::Scalar(String::new())),
        }
    }

    fn sequence(&mut self, indent: usize) -> Result<Value, ParseError> {
        let mut items = Vec::new();
        while let Some(found) = self.peek_indent() {
            if found != indent {
                break;
            }
            let content = &self.lines[self.position][indent..];
            if !is_dash(content) {
                break;
            }
            let item = content[1..].trim_start();
            let item_indent = indent + (content.len() - item.len());

            match split_key(item) {
                // `- key: value`: a map whose first entry shares the dash's line.
                Some((key, rest)) if !item.starts_with('{') && !item.starts_with('[') => {
                    self.position += 1;
                    let first = self.value_after_key(item_indent, rest)?;
                    let mut entries = vec![(key.to_string(), first)];
                    if self.peek_indent() == Some(item_indent)
                        && !is_dash(&self.lines[self.position][item_indent..])
                        && let Value::Map(more) = self.map(item_indent)?
                    {
                        entries.extend(more);
                    }
                    items.push(Value::Map(entries));
                }
                _ => {
                    self.position += 1;
                    items.push(self.inline(item)?);
                }
            }
        }
        Ok(Value::Seq(items))
    }

    /// A scalar, `{flow map}` or `[flow sequence]` written on one line.
    fn inline(&self, text: &str) -> Result<Value, ParseError> {
        let text = text.trim();
        if text.starts_with('{') || text.starts_with('[') {
            let mut flow = Flow {
                bytes: text.as_bytes(),
                position: 0,
            };
            flow.value().ok_or_else(|| self.error(format!("unreadable inline value `{text}`")))
        } else {
            Ok(Value::Scalar(unquote(text)))
        }
    }
}

fn is_dash(content: &str) -> bool {
    content == "-" || content.starts_with("- ")
}

/// `key: rest` or `key:` -> (key, rest).
fn split_key(content: &str) -> Option<(&str, &str)> {
    if content.starts_with('{') || content.starts_with('[') || content.starts_with('"') || content.starts_with('\'') {
        return None;
    }
    if let Some(position) = content.find(": ") {
        return Some((&content[..position], content[position + 2..].trim()));
    }
    content.strip_suffix(':').map(|key| (key, ""))
}

/// Whether `line` carries on the scalar `so_far` rather than starting a new key.
fn continues_scalar(so_far: &str, line: &str) -> bool {
    let open_quote = (so_far.starts_with('"') && !(so_far.len() > 1 && so_far.ends_with('"')))
        || (so_far.starts_with('\'') && !(so_far.len() > 1 && so_far.ends_with('\'')));
    let open_flow = so_far.starts_with('{') && !so_far.ends_with('}');
    open_quote || open_flow || (split_key(line).is_none() && !is_dash(line))
}

fn unquote(text: &str) -> String {
    let bytes = text.as_bytes();
    if bytes.len() >= 2 && bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\'' {
        text[1..text.len() - 1].replace("''", "'")
    } else if bytes.len() >= 2 && bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"' {
        text[1..text.len() - 1].replace("\\\"", "\"").replace("\\\\", "\\")
    } else {
        text.to_string()
    }
}

struct Flow<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl Flow<'_> {
    fn skip_spaces(&mut self) {
        while self.bytes.get(self.position) == Some(&b' ') {
            self.position += 1;
        }
    }

    fn value(&mut self) -> Option<Value> {
        self.skip_spaces();
        match self.bytes.get(self.position)? {
            b'{' => {
                self.position += 1;
                let mut entries = Vec::new();
                loop {
                    self.skip_spaces();
                    if self.bytes.get(self.position) == Some(&b'}') {
                        self.position += 1;
                        return Some(Value::Map(entries));
                    }
                    let key_start = self.position;
                    while *self.bytes.get(self.position)? != b':' {
                        self.position += 1;
                    }
                    let key = std::str::from_utf8(&self.bytes[key_start..self.position]).ok()?.trim();
                    self.position += 1;
                    let value = self.value()?;
                    entries.push((key.to_string(), value));
                    self.skip_spaces();
                    if self.bytes.get(self.position) == Some(&b',') {
                        self.position += 1;
                    }
                }
            }
            b'[' => {
                self.position += 1;
                let mut items = Vec::new();
                loop {
                    self.skip_spaces();
                    if self.bytes.get(self.position) == Some(&b']') {
                        self.position += 1;
                        return Some(Value::Seq(items));
                    }
                    items.push(self.value()?);
                    self.skip_spaces();
                    if self.bytes.get(self.position) == Some(&b',') {
                        self.position += 1;
                    }
                }
            }
            _ => {
                let start = self.position;
                while let Some(byte) = self.bytes.get(self.position) {
                    if matches!(byte, b',' | b'}' | b']') {
                        break;
                    }
                    self.position += 1;
                }
                let text = std::str::from_utf8(&self.bytes[start..self.position]).ok()?.trim();
                Some(Value::Scalar(unquote(text)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PREFAB: &str = "\
%YAML 1.1
%TAG !u! tag:unity3d.com,2011:
--- !u!1 &100
GameObject:
  m_Name: Light 2D (3)
  m_Component:
  - component: {fileID: 200}
  - component: {fileID: 300}
  m_IsActive: 1
--- !u!4 &200 stripped
Transform:
  m_LocalPosition: {x: 1.5, y: -2, z: 0}
  m_Children: []
--- !u!1839735485 &300
Tilemap:
  m_Tiles:
  - first: {x: 3, y: 4, z: 0}
    second:
      m_TileIndex: 7
      m_TileSpriteIndex: 2
  - first: {x: 5, y: 6, z: 0}
    second:
      m_TileIndex: 1
      m_TileSpriteIndex: 0
  m_AnimatedTiles: {}
  m_Path:
  - {x: -0.5, y: 0.5}
  - {x: 0.5, y: 0.5}
  m_Label: 'it''s'
  m_Empty:
  m_Count: 2
";

    #[test]
    fn reads_objects_headers_and_fields() {
        let documents = parse_documents(PREFAB).unwrap();
        assert_eq!(documents.len(), 3);

        let object = &documents[0];
        assert_eq!((object.file_id, object.stripped), (100, false));
        assert_eq!(object.class, "GameObject");
        assert_eq!(object.body.get("m_Name").str(), "Light 2D (3)");
        assert_eq!(object.body.get("m_Component").seq().len(), 2);
        assert_eq!(object.body.get("m_Component").seq()[1].get("component").get("fileID").i64(), 300);
        assert!(object.body.get("m_IsActive").bool());

        let transform = &documents[1];
        assert!(transform.stripped);
        assert_eq!(transform.body.get("m_LocalPosition").get("y").f32(), -2.0);
        assert_eq!(transform.body.get("m_Children").seq().len(), 0);
    }

    #[test]
    fn reads_sequences_of_maps_and_what_follows_them() {
        let documents = parse_documents(PREFAB).unwrap();
        let tilemap = &documents[2].body;
        let tiles = tilemap.get("m_Tiles").seq();
        assert_eq!(tiles.len(), 2);
        assert_eq!(tiles[0].get("first").get("x").i64(), 3);
        assert_eq!(tiles[1].get("second").get("m_TileIndex").i64(), 1);
        // Keys after the sequence still belong to the parent map.
        assert_eq!(tilemap.get("m_Path").seq()[1].get("x").f32(), 0.5);
        assert_eq!(tilemap.get("m_Label").str(), "it's");
        assert_eq!(tilemap.get("m_Empty").str(), "");
        assert_eq!(tilemap.get("m_Count").i64(), 2);
        assert!(tilemap.has("m_AnimatedTiles") && !tilemap.has("m_Missing"));
    }

    #[test]
    fn missing_keys_chain_to_defaults() {
        let value = parse_map("a:\n  b: 2\n").unwrap();
        assert_eq!(value.get("a").get("b").i64(), 2);
        assert_eq!(value.get("x").get("y").get("z").f32(), 0.0);
        assert_eq!(value.get("a").get("b").get("c").str(), "");
    }

    #[test]
    fn reads_meta_files_with_nested_sequences() {
        let meta = "\
fileFormatVersion: 2
guid: abc123
TextureImporter:
  spriteSheet:
    sprites:
    - serializedVersion: 2
      name: tiles_0
      rect:
        serializedVersion: 2
        x: 0
        y: 160
        width: 32
        height: 32
      internalID: -12345
    - serializedVersion: 2
      name: tiles_1
      rect:
        x: 32
        y: 160
      internalID: 678
    outline: []
  spritePixelsToUnits: 64
";
        let value = parse_map(meta).unwrap();
        assert_eq!(value.get("guid").str(), "abc123");
        let importer = value.get("TextureImporter");
        let sprites = importer.get("spriteSheet").get("sprites").seq();
        assert_eq!(sprites.len(), 2);
        assert_eq!(sprites[0].get("rect").get("y").i64(), 160);
        assert_eq!(sprites[0].get("internalID").i64(), -12345);
        assert_eq!(sprites[1].get("name").str(), "tiles_1");
        assert_eq!(importer.get("spritePixelsToUnits").f32(), 64.0);
    }

    #[test]
    fn set_path_overrides_fields_and_array_elements() {
        let mut value = parse_map("m_Color: {r: 1, g: 1}\nm_Layers: b1f58d25\n").unwrap();
        value.set_path("m_Color.r", "0.5");
        value.set_path("m_Intensity", "3");
        assert_eq!(value.get("m_Color").get("r").f32(), 0.5);
        assert_eq!(value.get("m_Color").get("g").f32(), 1.0);
        assert_eq!(value.get("m_Intensity").i64(), 3);

        // Overriding an array replaces whatever form it was stored in.
        value.set_path("m_Layers.Array.size", "2");
        value.set_path("m_Layers.Array.data[0]", "7");
        value.set_path("m_Layers.Array.data[1]", "9");
        let layers: Vec<i64> = value.get("m_Layers").seq().iter().map(Value::i64).collect();
        assert_eq!(layers, [7, 9]);
    }

    #[test]
    fn long_scalars_wrap_onto_deeper_lines() {
        let value = parse_map("a: one two\n    three\nb: 1\n").unwrap();
        assert_eq!(value.get("a").str(), "one two three");
        assert_eq!(value.get("b").i64(), 1);
    }
}
