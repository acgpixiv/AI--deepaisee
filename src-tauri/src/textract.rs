//! 文本抽取：纯文本 / RTF / Office(OOXML + 97-2003 OLE2) / PDF / EPUB。
use std::io::Read;
use std::path::Path;

pub const MAX_CHARS_DEFAULT: usize = 200_000;

/// 纯文本类扩展名：直接按字节读 + 编码嗅探。
const PLAIN_EXTS: &[&str] = &[
    "txt", "md", "markdown", "log", "csv", "tsv", "json", "jsonl", "xml", "yaml", "yml", "toml",
    "ini", "cfg", "conf", "html", "htm", "css", "js", "jsx", "ts", "tsx", "vue", "py", "rs", "go",
    "java", "c", "h", "cpp", "hpp", "cs", "php", "rb", "sh", "bat", "ps1", "sql", "lua", "kt",
    "swift", "m", "r", "jl", "tex", "rst", "srt", "ass", "vtt", "nfo", "diff", "patch", "sln",
    "csproj", "gradle", "properties", "env", "gitignore",
];

/// 返回正文。不可抽取的类型返回 None。
pub fn extract_text(path: &Path, max_chars: usize) -> Option<String> {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if PLAIN_EXTS.contains(&ext.as_str()) {
        return Some(read_plain(path, max_chars));
    }
    let text = match ext.as_str() {
        "docx" => ooxml_text(path, "word/document.xml", "w:t", max_chars),
        "pptx" => ooxml_slides(path, max_chars),
        "xlsx" => ooxml_xlsx(path, max_chars),
        "doc" => ole_doc_text(path, max_chars),
        "xls" => ole_xls_text(path, max_chars),
        "ppt" => ole_ppt_text(path, max_chars),
        "odt" | "ods" | "odp" | "odg" => opendocument_text(path, max_chars),
        "pdf" => pdf_text(path, max_chars),
        "epub" => epub_text(path, max_chars),
        "rtf" => rtf_text(path, max_chars),
        _ => return None,
    };
    Some(text)
}

fn read_plain(path: &Path, max_chars: usize) -> String {
    match std::fs::read(path) {
        Ok(b) => truncate_chars(&decode_bytes(&b), max_chars),
        Err(_) => String::new(),
    }
}

/// 依次尝试 BOM / UTF-8 / GB18030 / 有损 UTF-8。
pub fn decode_bytes(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(rest).to_string();
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return encoding_rs::UTF_16LE.decode(rest).0.to_string();
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return encoding_rs::UTF_16BE.decode(rest).0.to_string();
    }
    if let Ok(s) = std::str::from_utf8(bytes) {
        return s.to_string();
    }
    let (s, _, had_errors) = encoding_rs::GB18030.decode(bytes);
    if !had_errors || s.chars().filter(|c| *c == '\u{FFFD}').count() < s.len() / 20 {
        s.to_string()
    } else {
        String::from_utf8_lossy(bytes).to_string()
    }
}

pub fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    text.chars().take(max_chars).collect()
}

fn keep_printable(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut i = 0usize;
    let mut last_nl = false;
    for ch in s.chars() {
        // Word 97 用 \r 分段、\x07 表示单元格结束，直接丢掉会让整篇正文粘成一串
        if ch == '\r' || ch == '\u{7}' || ch == '\u{b}' {
            if !last_nl {
                out.push('\n');
                last_nl = true;
                i += 1;
            }
            continue;
        }
        if ch == '\n' || ch == '\t' || !ch.is_control() {
            out.push(ch);
            last_nl = ch == '\n';
            i += 1;
            if i >= 4000 {
                break;
            }
        }
    }
    out
}

// ---------------------------------------------------------------- OOXML / zip
fn open_zip(path: &Path) -> Option<zip::ZipArchive<std::fs::File>> {
    zip::ZipArchive::new(std::fs::File::open(path).ok()?).ok()
}

fn read_zip_entry<R: Read + std::io::Seek>(
    zip: &mut zip::ZipArchive<R>,
    name: &str,
    max: usize,
) -> Option<String> {
    let f = zip.by_name(name).ok()?;
    let mut buf = Vec::new();
    f.take((max * 4) as u64).read_to_end(&mut buf).ok()?;
    Some(decode_bytes(&buf))
}

/// 抽出指定标签内的文本。
fn xml_tag_text(xml: &str, tag: &str) -> String {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut out = String::new();
    let mut rest = xml;
    while let Some(i) = rest.find(&open) {
        let after = &rest[i..];
        let Some(gt) = after.find('>') else { break };
        let body = &after[gt + 1..];
        let Some(end) = body.find(&close) else { break };
        out.push_str(&xml_unescape(&body[..end]));
        rest = &body[end + close.len()..];
    }
    out
}

fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

fn ooxml_text(path: &Path, entry: &str, tag: &str, max: usize) -> String {
    let Some(mut zip) = open_zip(path) else { return String::new() };
    let Some(xml) = read_zip_entry(&mut zip, entry, max) else { return String::new() };
    truncate_chars(&xml_paragraph_text(&xml, tag, "</w:p>"), max)
}

/// 按段落抽文本：整篇 `xml_tag_text` 会把所有段落粘成一长串，
/// 「项目周报 第42周本周完成…」这种连读在片段预览里很难看，也不利于
/// 按段落做命中定位。按段尾标签切开，段内抽 tag，段间补换行。
fn xml_paragraph_text(xml: &str, tag: &str, para_end: &str) -> String {
    let mut out = String::new();
    let mut rest = xml;
    while let Some(i) = rest.find(para_end) {
        let (line, tail) = rest.split_at(i + para_end.len());
        let t = xml_tag_text(line, tag);
        if !t.trim().is_empty() {
            out.push_str(&t);
            out.push('\n');
        }
        rest = tail;
    }
    // 最后一小段（没有段落结束标签的残缺 xml）
    let t = xml_tag_text(rest, tag);
    if !t.trim().is_empty() {
        out.push_str(&t);
        out.push('\n');
    }
    out
}

/// 遍历 zip 内匹配前缀的 xml 条目，抽取 tag 文本（pptx 幻灯片 / odt 等复用）。
fn zip_entries_text<F: Fn(&str) -> bool>(
    path: &Path,
    match_fn: F,
    tag: &str,
    max: usize,
) -> String {
    let Some(mut zip) = open_zip(path) else { return String::new() };
    let mut names: Vec<String> = zip
        .file_names()
        .filter(|n| match_fn(n))
        .map(|s| s.to_string())
        .collect();
    names.sort();
    let mut out = String::new();
    for n in names {
        if let Some(xml) = read_zip_entry(&mut zip, &n, max) {
            out.push_str(&xml_tag_text(&xml, tag));
            out.push('\n');
        }
    }
    truncate_chars(&out, max)
}

fn ooxml_slides(path: &Path, max: usize) -> String {
    zip_entries_text(
        path,
        |n| n.starts_with("ppt/slides/slide") && n.ends_with(".xml"),
        "a:t",
        max,
    )
}

fn ooxml_xlsx(path: &Path, max: usize) -> String {
    let Some(mut zip) = open_zip(path) else { return String::new() };
    let mut shared: Vec<String> = Vec::new();
    if let Some(xml) = read_zip_entry(&mut zip, "xl/sharedStrings.xml", max) {
        let mut rest = xml.as_str();
        while let Some(i) = rest.find("<si>") {
            let body = &rest[i + 4..];
            let Some(end) = body.find("</si>") else { break };
            shared.push(xml_tag_text(&body[..end], "t"));
            rest = &body[end + 5..];
        }
    }
    let mut names: Vec<String> = zip
        .file_names()
        .filter(|n| n.starts_with("xl/worksheets/sheet") && n.ends_with(".xml"))
        .map(|s| s.to_string())
        .collect();
    names.sort();
    let mut out = String::new();
    for n in names {
        if let Some(xml) = read_zip_entry(&mut zip, &n, max) {
            let mut rest = xml.as_str();
            while let Some(i) = rest.find("<c") {
                let body = &rest[i..];
                let Some(end) = body.find("</c>") else { break };
                let cell = &body[..end];
                if cell.contains("t=\"s\"") {
                    if let Some(idx) = cell
                        .find("<v>")
                        .and_then(|p| cell[p + 3..].find("</v>").map(|e| &cell[p + 3..p + 3 + e]))
                        .and_then(|v| v.trim().parse::<usize>().ok())
                    {
                        if let Some(s) = shared.get(idx) {
                            out.push_str(s);
                            out.push('\t');
                        }
                    }
                } else if cell.contains("t=\"inlineStr\"") || cell.contains("<is>") {
                    out.push_str(&xml_tag_text(cell, "t"));
                    out.push('\t');
                } else {
                    out.push_str(&xml_tag_text(cell, "v"));
                    out.push('\t');
                }
                rest = &body[end + 4..];
            }
            out.push('\n');
        }
    }
    truncate_chars(&out, max)
}

fn opendocument_text(path: &Path, max: usize) -> String {
    let Some(mut zip) = open_zip(path) else { return String::new() };
    let Some(xml) = read_zip_entry(&mut zip, "content.xml", max) else { return String::new() };
    // 段落/单元格/标题之间补分隔，否则所有文字粘成一坨
    let spaced = xml
        .replace("</text:p>", "\n")
        .replace("</text:h>", "\n")
        .replace("</table:table-cell>", "\t");
    truncate_chars(&strip_html(&spaced), max)
}

fn pdf_text(path: &Path, max: usize) -> String {
    // Scanned PDFs often contain only image draw operations. Avoid sending
    // those files through pdf-extract: it walks every embedded font and can
    // emit thousands of harmless "unknown glyph name" lines for Dingbats and
    // damaged font maps. A page content stream with a text operator is enough
    // to justify the more expensive extractor.
    if !pdf_has_text_operator(path) {
        return String::new();
    }
    // pdf-extract has a few malformed-font assertions. A damaged PDF should
    // become an empty extraction result, never unwind the index worker.
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        pdf_extract::extract_text(path)
            .map(|t| truncate_chars(&t, max))
            .unwrap_or_default()
    }))
    .unwrap_or_default()
}

fn pdf_has_text_operator(path: &Path) -> bool {
    let Ok(doc) = lopdf::Document::load(path) else {
        // Keep the previous best-effort behavior when lopdf cannot parse a
        // damaged file; pdf-extract may still recover useful text.
        return true;
    };
    for page_id in doc.page_iter() {
        let Ok(content) = doc.get_page_content(page_id) else { continue };
        let has = content
            .split(|byte| byte.is_ascii_whitespace())
            .any(|token| matches!(token, b"Tj" | b"TJ" | b"'" | b"\""));
        if has {
            return true;
        }
    }
    false
}

/// Decode common embedded PDF raster images for multimodal indexing. JPEG
/// streams are passed through directly; simple 8-bit RGB/gray Flate streams
/// are reconstructed from their declared dimensions. Unsupported PDF image
/// encodings are skipped without failing the document.
pub fn pdf_images(path: &Path, limit: usize) -> Vec<image::DynamicImage> {
    pdf_images_with_cancel(path, limit, || false)
}

/// Same decoder with a cooperative cancellation hook. PDF decompression itself
/// is library code and cannot be interrupted, but checking between pages and
/// embedded images keeps a large scanned document from delaying cancellation.
pub fn pdf_images_with_cancel<F>(path: &Path, limit: usize, cancelled: F) -> Vec<image::DynamicImage>
where
    F: Fn() -> bool,
{
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let Ok(doc) = lopdf::Document::load(path) else { return Vec::new() };
        let mut out = Vec::new();
        for page_id in doc.page_iter() {
            if cancelled() { return out; }
            let Ok(images) = doc.get_page_images(page_id) else { continue };
            for item in images {
                if cancelled() { return out; }
                if out.len() >= limit { return out; }
                let filters = item.filters.clone().unwrap_or_default();
                let bytes = if filters.iter().any(|f| f.eq_ignore_ascii_case("DCTDecode")) {
                    item.content.to_vec()
                } else {
                    let mut dict = item.origin_dict.clone();
                    dict.remove(b"Subtype");
                    let stream = lopdf::Stream::new(dict, item.content.to_vec());
                    match stream.decompressed_content() { Ok(v) => v, Err(_) => continue }
                };
                if filters.iter().any(|f| f.eq_ignore_ascii_case("DCTDecode")) {
                    if let Ok(img) = image::load_from_memory(&bytes) { out.push(img.thumbnail(1024, 1024)); }
                    continue;
                }
                if item.width <= 0 || item.height <= 0 || item.width > 20_000 || item.height > 20_000
                    || item.width.saturating_mul(item.height) > 100_000_000 { continue; }
                let width = item.width.max(1) as u32;
                let height = item.height.max(1) as u32;
                let color = item.color_space.as_deref().unwrap_or("").to_ascii_lowercase();
                let image = if color.contains("gray") && bytes.len() >= (width as usize * height as usize) {
                    image::GrayImage::from_raw(width, height, bytes[..width as usize * height as usize].to_vec())
                        .map(image::DynamicImage::ImageLuma8)
                } else if color.contains("rgb") && bytes.len() >= (width as usize * height as usize * 3) {
                    image::RgbImage::from_raw(width, height, bytes[..width as usize * height as usize * 3].to_vec())
                        .map(image::DynamicImage::ImageRgb8)
                } else { None };
                if let Some(img) = image { out.push(img.thumbnail(1024, 1024)); }
            }
        }
        out
    })).unwrap_or_default()
}

fn epub_text(path: &Path, max: usize) -> String {
    let Some(mut zip) = open_zip(path) else { return String::new() };
    let mut names: Vec<String> = zip
        .file_names()
        .filter(|n| n.ends_with(".xhtml") || n.ends_with(".html") || n.ends_with(".htm"))
        .map(|s| s.to_string())
        .collect();
    names.sort();
    let mut out = String::new();
    for n in names {
        if let Some(html) = read_zip_entry(&mut zip, &n, max) {
            out.push_str(&strip_html(&html));
            out.push('\n');
        }
    }
    truncate_chars(&out, max)
}

fn strip_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len() / 2);
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    xml_unescape(&out)
}

// ---------------------------------------------------------------- RTF
/// 目标组：这些控制词后面紧跟的 `{...}` 是元数据（字体表、样式表、图片、
/// 文档信息），不是正文，整组跳过。
const SKIP_GROUPS: &[&str] = &[
    "fonttbl", "colortbl", "stylesheet", "info", "pict", "object", "header", "footer",
    "themedata", "colorschememapping", "latentstyles", "datastore", "listtable", "listoverridetable",
];

/// 极简 RTF：丢弃控制字，处理 \uN / \'hh 转义。
///
/// 字体表 / 样式表 / 图片等「目标组」整组跳过：不跳的话，字体名
/// （`Helvetica;`）会被当成正文塞进索引，搜 "helvet" 反而命中一堆文档。
fn rtf_text(path: &Path, max: usize) -> String {
    let Ok(bytes) = std::fs::read(path) else { return String::new() };
    let src = String::from_utf8_lossy(&bytes);
    let mut out = String::new();
    let mut chars = src.chars().peekable();
    let mut pending_hi: Option<u8> = None;
    let mut depth: usize = 0;
    let mut skip_to: Option<usize> = None; // 跳到该深度的 `}` 为止
    while let Some(c) = chars.next() {
        if let Some(d) = skip_to {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth = depth.saturating_sub(1);
                    if depth < d {
                        skip_to = None;
                    }
                }
                '\\' => {
                    chars.next();
                }
                _ => {}
            }
            continue;
        }
        match c {
            '\\' => match chars.next() {
                Some('u') => {
                    let mut num = String::new();
                    if chars.peek() == Some(&'-') {
                        num.push('-');
                        chars.next();
                    }
                    while let Some(d) = chars.peek().filter(|d| d.is_ascii_digit()) {
                        num.push(*d);
                        chars.next();
                    }
                    if let Ok(v) = num.parse::<i32>() {
                        if let Some(ch) = char::from_u32(if v < 0 { v + 65536 } else { v } as u32) {
                            out.push(ch);
                        }
                    }
                    // 跳过 \ucN 指定的占位字符（通常 1 个）
                    chars.next();
                }
                Some('\'') => {
                    let hi = chars.next().and_then(|a| chars.next().and_then(|b| {
                        u8::from_str_radix(&format!("{a}{b}"), 16).ok()
                    }));
                    if let Some(b) = hi {
                        // 两字节 GBK 组合
                        match (pending_hi, b) {
                            (Some(h), _) if h >= 0x81 => {
                                let pair = [h, b];
                                out.push_str(&decode_bytes(&pair));
                                pending_hi = None;
                            }
                            _ if b >= 0x81 => pending_hi = Some(b),
                            _ => out.push(b as char),
                        }
                    }
                }
                Some(other) => {
                    // 控制词：字母序列 + 可选数字 + 可选空格
                    let mut word = String::new();
                    word.push(other);
                    let mut is_alpha = other.is_ascii_alphabetic();
                    while let Some(&n) = chars.peek() {
                        if n.is_ascii_alphabetic() && is_alpha {
                            word.push(n);
                            chars.next();
                        } else if n.is_ascii_digit() || n == '-' {
                            is_alpha = false;
                            chars.next();
                        } else {
                            break;
                        }
                    }
                    if chars.peek() == Some(&' ') {
                        chars.next();
                    }
                    let w = word.as_str();
                    if SKIP_GROUPS.iter().any(|g| w == *g) {
                        // 接下来这一组是字体表/样式表/图片，整组丢弃
                        while let Some(&n) = chars.peek() {
                            if n == '{' {
                                depth += 1;
                                chars.next();
                                break;
                            }
                            chars.next();
                        }
                        skip_to = Some(depth);
                        continue;
                    }
                    if w.starts_with("par") || w.starts_with("line") || w.starts_with("row") {
                        out.push('\n');
                    } else if w.starts_with("tab") || w.starts_with("cell") {
                        out.push('\t');
                    }
                }
                None => break,
            },
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            '\r' | '\n' => {}
            c => out.push(c),
        }
        if out.chars().count() >= max {
            break;
        }
    }
    truncate_chars(&keep_printable(&out), max)
}

// ---------------------------------------------------------------- OLE2 (97-2003)
//
// Word/Excel/PowerPoint 97-2003 都是 OLE2 复合文档（CFB）。
// 只用「读根目录 → 按名字找流 → 读 MiniFAT/FAT 链」这一小撮能力，
// 自己实现约 120 行，比引入 cfb + fnv + uuid 三个 crate 更划算。

struct Ole {
    data: Vec<u8>,
    sector_size: usize,
    mini_size: usize,
    mini_cutoff: usize,
    fat: Vec<u32>,
    mini_fat: Vec<u32>,
    mini_stream: Vec<u8>,
    entries: Vec<(String, u8, u32, u64)>, // 名称 / 类型 / 起始 / 长度
}

fn ole_open(path: &Path) -> Option<Ole> {
    let data = std::fs::read(path).ok()?;
    if data.len() < 512 || &data[0..8] != b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1" {
        return None;
    }
    let sector_shift = u16_at(&data, 0x1E) as usize;
    let mini_shift = u16_at(&data, 0x20) as usize;
    let sector_size = 1usize << sector_shift;
    let mini_size = 1usize << mini_shift;
    let n_fat = u32_at(&data, 0x2C) as usize;
    let dir_start = u32_at(&data, 0x30);
    let mini_cutoff = u32_at(&data, 0x38) as usize;
    let mini_fat_start = u32_at(&data, 0x3C);
    let n_mini_fat = u32_at(&data, 0x40) as usize;
    let difat_start = u32_at(&data, 0x44);
    let n_difat = u32_at(&data, 0x48) as usize;

    let sector = |i: u32| -> Option<&[u8]> {
        let off = (i as usize + 1) * sector_size;
        data.get(off..off + sector_size)
    };

    // DIFAT：头部 109 项 + 后续 DIFAT 扇区
    let mut fat_sectors: Vec<u32> = Vec::new();
    for i in 0..109usize {
        if fat_sectors.len() >= n_fat {
            break;
        }
        let v = u32_at(&data, 0x4C + i * 4);
        if v != 0xFFFF_FFFF {
            fat_sectors.push(v);
        }
    }
    let mut d = difat_start;
    let mut guard = 0;
    while n_difat > 0 && d != 0xFFFF_FFFF && guard < n_difat + 8 {
        guard += 1;
        let Some(sec) = sector(d) else { break };
        let per = sector_size / 4 - 1;
        for i in 0..per {
            let v = u32_at(sec, i * 4);
            if v != 0xFFFF_FFFF && fat_sectors.len() < n_fat {
                fat_sectors.push(v);
            }
        }
        d = u32_at(sec, per * 4);
    }

    let mut fat: Vec<u32> = Vec::new();
    for s in fat_sectors {
        let Some(sec) = sector(s) else { continue };
        for i in 0..sector_size / 4 {
            fat.push(u32_at(sec, i * 4));
        }
    }
    if fat.is_empty() {
        return None;
    }

    let read_chain = |start: u32, limit_secs: usize| -> Vec<u8> {
        let mut out = Vec::new();
        let mut cur = start;
        let mut guard = 0usize;
        while cur != 0xFFFF_FFFF && cur != 0xFFFF_FFFE && guard < limit_secs.max(1) {
            let Some(sec) = sector(cur) else { break };
            out.extend_from_slice(sec);
            cur = *fat.get(cur as usize).unwrap_or(&0xFFFF_FFFF);
            guard += 1;
        }
        out
    };

    // 目录项：128 字节一条，找根（类型 5）与它的 mini stream 起点
    let dir = read_chain(dir_start, n_fat + 16);
    let mut entries = Vec::new();
    let mut root = None;
    for i in 0..dir.len() / 128 {
        let e = &dir[i * 128..(i + 1) * 128];
        let name_len = u16_at(e, 0x40).min(64) as usize;
        let name: String = e[0..name_len.saturating_sub(2).max(0)]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .filter(|c| *c != 0)
            .map(|c| char::from_u32(c as u32).unwrap_or('?'))
            .collect();
        let kind = e[0x42];
        let first = u32_at(e, 0x74);
        let len = u64::from_le_bytes(e[0x78..0x80].try_into().ok()?) as u64;
        if kind == 5 {
            root = Some((first, len));
        }
        entries.push((name, kind, first, len));
    }

    let mini_stream = match root {
        Some((first, _)) => read_chain(first, n_fat + 16),
        None => Vec::new(),
    };
    let mut mini_fat = Vec::new();
    if n_mini_fat > 0 {
        let raw = read_chain(mini_fat_start, n_mini_fat);
        for i in 0..raw.len() / 4 {
            mini_fat.push(u32_at(&raw, i * 4));
        }
    }
    Some(Ole { data, sector_size, mini_size, mini_cutoff, fat, mini_fat, mini_stream, entries })
}

impl Ole {
    /// 读流：小于 mini_cutoff 的走 MiniFAT，否则走 FAT 链。
    fn stream(&self, name: &str, max: usize) -> Option<Vec<u8>> {
        let (_, _, start, len) = self
            .entries
            .iter()
            .find(|e| e.1 == 2 && e.0.eq_ignore_ascii_case(name))?
            .clone();
        let len = (len as usize).min(max * 8).min(64 << 20);
        let mut out = Vec::with_capacity(len.min(1 << 20));
        if len < self.mini_cutoff {
            let mut cur = start;
            let mut guard = 0usize;
            while cur != 0xFFFF_FFFF && guard < len / self.mini_size.max(1) + 2 {
                let off = cur as usize * self.mini_size;
                let sec = self.mini_stream.get(off..off + self.mini_size)?;
                out.extend_from_slice(sec);
                cur = *self.mini_fat.get(cur as usize).unwrap_or(&0xFFFF_FFFF);
                guard += 1;
            }
        } else {
            let mut cur = start;
            let mut guard = 0usize;
            while cur != 0xFFFF_FFFF && guard < len / self.sector_size.max(1) + 2 {
                let off = (cur as usize + 1) * self.sector_size;
                let sec = self.data.get(off..off + self.sector_size)?;
                out.extend_from_slice(sec);
                cur = *self.fat.get(cur as usize).unwrap_or(&0xFFFF_FFFF);
                guard += 1;
            }
        }
        out.truncate(len);
        Some(out)
    }
}

fn u16_at(b: &[u8], o: usize) -> u16 {
    b.get(o..o + 2)
        .map(|x| u16::from_le_bytes([x[0], x[1]]))
        .unwrap_or(0)
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    b.get(o..o + 4)
        .map(|x| u32::from_le_bytes([x[0], x[1], x[2], x[3]]))
        .unwrap_or(0xFFFF_FFFF)
}

/// Word 97-2003：FIB → Clx → piece table（每块可能是 UTF-16 或单字节）。
fn ole_doc_text(path: &Path, max: usize) -> String {
    let Some(cf) = ole_open(path) else { return String::new() };
    let Some(doc) = cf.stream("WordDocument", max) else { return String::new() };
    if u16_at(&doc, 0) != 0xA5EC {
        return String::new();
    }
    let table = if u16_at(&doc, 0x0A) & 0x0200 != 0 { "1Table" } else { "0Table" };
    let Some(tbl) = cf.stream(table, max) else { return String::new() };
    // fibRgFcLcb 起始 0x9A，fcClx 是第 34 项 → 0x9A + 33*8
    let mut fc = u32_at(&doc, 0x1A2) as usize;
    let mut lcb = u32_at(&doc, 0x1A6) as usize;
    if lcb == 0 || fc + lcb > tbl.len() {
        // 布局异常时按 FIB 里声明的 cbRgFcLcb 逐项找
        let cb = u16_at(&doc, 0x5A) as usize;
        if cb > 33 {
            let base = 0x9A;
            fc = u32_at(&doc, base + 33 * 8) as usize;
            lcb = u32_at(&doc, base + 33 * 8 + 4) as usize;
        }
    }
    if lcb == 0 || fc + lcb > tbl.len() {
        return String::new();
    }
    let clx = &tbl[fc..fc + lcb];
    let mut i = 0usize;
    while i + 3 <= clx.len() && clx[i] == 0x01 {
        i += 3 + u16_at(clx, i + 1) as usize;
    }
    if i + 5 > clx.len() || clx[i] != 0x02 {
        return String::new();
    }
    let plc_len = u32_at(clx, i + 1) as usize;
    let plc = &clx[i + 5..(i + 5 + plc_len).min(clx.len())];
    if plc.len() < 16 {
        return String::new();
    }
    let n = (plc.len() - 4) / 12;
    let mut out = String::new();
    for k in 0..n {
        let cp0 = u32_at(plc, k * 4) as usize;
        let cp1 = u32_at(plc, k * 4 + 4) as usize;
        let pcd = (n + 1) * 4 + k * 8;
        if pcd + 8 > plc.len() {
            break;
        }
        let fc_raw = u32_at(plc, pcd + 2);
        let len = cp1.saturating_sub(cp0);
        if fc_raw & 0x4000_0000 != 0 {
            let start = ((fc_raw & 0x3FFF_FFFF) / 2) as usize;
            if let Some(bytes) = doc.get(start..start + len) {
                out.push_str(&decode_bytes(bytes));
            }
        } else {
            let start = (fc_raw & 0x3FFF_FFFF) as usize;
            if let Some(bytes) = doc.get(start..start + len * 2) {
                let units: Vec<u16> = bytes
                    .chunks_exact(2)
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .collect();
                out.push_str(&String::from_utf16_lossy(&units));
            }
        }
        if out.chars().count() >= max {
            break;
        }
    }
    truncate_chars(&keep_printable(&out), max)
}

/// Excel 97-2003 (BIFF8)：SST 共享字符串 + 数字单元。
fn ole_xls_text(path: &Path, max: usize) -> String {
    let Some(cf) = ole_open(path) else { return String::new() };
    let Some(data) = cf.stream("Workbook", max).or_else(|| cf.stream("Book", max)) else {
        return String::new();
    };
    let mut sst: Vec<String> = Vec::new();
    let mut out = String::new();
    let mut i = 0usize;
    while i + 4 <= data.len() {
        let typ = u16_at(&data, i);
        let len = u16_at(&data, i + 2) as usize;
        let body = &data[i + 4..(i + 4 + len).min(data.len())];
        match typ {
            0x00FC => parse_sst(&data, i + 4, len, &mut sst),
            0x00FD => {
                let idx = u32_at(body, 6) as usize;
                if let Some(s) = sst.get(idx) {
                    out.push_str(s);
                    out.push('\t');
                }
            }
            // NUMBER / RK / MULRK 里的数值：表格正文往往就是这些数字
            0x0203 => out.push_str(&format!("{}\t", f64_at(body, 6))),
            0x027E => out.push_str(&format!("{}\t", rk_value(u32_at(body, 6)))),
            0x000A => out.push('\n'), // EOF
            _ => {}
        }
        i += 4 + len;
        if out.chars().count() >= max {
            break;
        }
    }
    truncate_chars(&keep_printable(&out), max)
}

fn f64_at(b: &[u8], o: usize) -> f64 {
    b.get(o..o + 8)
        .map(|x| f64::from_le_bytes([x[0], x[1], x[2], x[3], x[4], x[5], x[6], x[7]]))
        .unwrap_or(0.0)
}

fn rk_value(raw: u32) -> f64 {
    let v = if raw & 0x02 != 0 {
        let n = (raw as i32) >> 2;
        n as f64
    } else {
        f64::from_bits(((raw & 0xFFFF_FFFC) as u64) << 32)
    };
    if raw & 0x01 != 0 {
        v / 100.0
    } else {
        v
    }
}

/// 解析 SST 记录（含其后的 CONTINUE）。跨记录断开的字符串直接跳过。
fn parse_sst(data: &[u8], start: usize, len: usize, out: &mut Vec<String>) {
    let end = (start + len).min(data.len());
    let mut p = start + 8; // 跳过 cstTotal / cstUnique
    while p + 3 <= end {
        let cch = u16_at(data, p) as usize;
        let flags = data[p + 2];
        let mut q = p + 3;
        let wide = flags & 0x01 != 0;
        let rich = flags & 0x08 != 0;
        let ext = flags & 0x04 != 0;
        let mut skip = if wide { cch * 2 } else { cch };
        if rich {
            skip += 2 + 4 * u16_at(data, q) as usize;
            q += 2;
        }
        if ext {
            skip += 4 + u32_at(data, q) as usize;
            q += 4;
        }
        if q + skip > end {
            // 跨 CONTINUE：放弃这条，跳到下一个记录边界由外层继续
            break;
        }
        let bytes = &data[q..q + skip];
        let s = if wide {
            let units: Vec<u16> = bytes
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            String::from_utf16_lossy(&units)
        } else {
            decode_bytes(bytes)
        };
        out.push(s);
        p = q + skip;
        if out.len() > 200_000 {
            break;
        }
    }
}

/// PowerPoint 97-2003：遍历记录树，取 TextCharsAtom / TextBytesAtom。
fn ole_ppt_text(path: &Path, max: usize) -> String {
    let Some(cf) = ole_open(path) else { return String::new() };
    let Some(data) = cf.stream("PowerPoint Document", max) else { return String::new() };
    let mut out = String::new();
    let mut stack: Vec<(usize, usize)> = vec![(0, data.len())];
    while let Some((mut p, end)) = stack.pop() {
        while p + 8 <= end {
            let ver_inst = u16_at(&data, p);
            let typ = u16_at(&data, p + 2);
            let len = u32_at(&data, p + 4) as usize;
            let body_start = p + 8;
            let body_end = (body_start + len).min(end);
            if body_end < body_start {
                break;
            }
            let container = ver_inst & 0x000F == 0x000F;
            match typ {
                0x0FA0 => {
                    let units: Vec<u16> = data[body_start..body_end]
                        .chunks_exact(2)
                        .map(|c| u16::from_le_bytes([c[0], c[1]]))
                        .collect();
                    out.push_str(&String::from_utf16_lossy(&units));
                    out.push('\n');
                }
                0x0FA8 => {
                    out.push_str(&decode_bytes(&data[body_start..body_end]));
                    out.push('\n');
                }
                _ => {}
            }
            if container && body_end > body_start {
                stack.push((body_start, body_end));
            }
            p = body_end;
            if out.chars().count() >= max {
                break;
            }
        }
    }
    truncate_chars(&keep_printable(&out), max)
}

// ---------------------------------------------------------------- 自检
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_exts_decode() {
        assert_eq!(truncate_chars("abcdef", 3), "abc");
        assert_eq!(decode_bytes(b"hello"), "hello");
    }

    #[test]
    fn rtf_strips_controls() {
        assert_eq!(rtf_text_of(r"{\rtf1\ansi 你好\par 世界}"), "你好\n世界");
    }

    fn rtf_text_of(s: &str) -> String {
        let dir = std::env::temp_dir().join("deepdark-oracle-rtf-test.rtf");
        std::fs::write(&dir, s).unwrap();
        let t = rtf_text(&dir, 1000);
        let _ = std::fs::remove_file(&dir);
        t
    }
}
