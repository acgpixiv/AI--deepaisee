//! RoBERTa 风格的字节级 BPE 分词（CLAP 文本塔用）。
//!
//! 为什么不用 `clip::bert_encode`：CLAP 的文本塔是 RoBERTa，词表是
//! `vocab.json` + `merges.txt` 的 BPE，和 BERT 的 `vocab.txt` 完全是两套。
//! 拿 BERT 的词表去喂，token id 会整体错位，出来的向量是噪声。
//!
//! 实现只覆盖推理需要的最小集：ByteLevel 预切分 → 字节映射 → 贪心合并。
//! 不做 Normalizer（CLAP 的 tokenizer.json 里 `normalizer: null`）。
use std::collections::HashMap;
use std::path::Path;

/// GPT-2 的字节 → 可见字符映射（让所有 256 个字节都能当词表键）。
fn bytes_to_unicode() -> [char; 256] {
    let mut table = ['\0'; 256];
    let mut n = 0u32;
    for b in 0u16..=255 {
        let c = b as u8;
        // 可打印 ASCII、Latin-1 的字母区直接映射；其余顺延到 256+
        let keep = (0x21..=0x7E).contains(&c) || (0xA1..=0xAC).contains(&c) || (0xAE..=0xFF).contains(&c);
        if keep {
            table[c as usize] = c as char;
        } else {
            table[c as usize] = char::from_u32(256 + n).unwrap_or('\u{fffd}');
            n += 1;
        }
    }
    table
}

pub struct Bpe {
    vocab: HashMap<String, i64>,
    ranks: HashMap<(String, String), usize>,
    byte_map: [char; 256],
}

impl Bpe {
    /// `vocab.json` + `merges.txt`
    pub fn load(dir: &Path) -> Option<Bpe> {
        let vocab_txt = std::fs::read_to_string(dir.join("vocab.json")).ok()?;
        let vocab: HashMap<String, i64> = serde_json::from_str(&vocab_txt).ok()?;
        let merges = std::fs::read_to_string(dir.join("merges.txt")).ok()?;
        let mut ranks = HashMap::with_capacity(50_000);
        for (i, line) in merges.lines().enumerate() {
            // 首行是 `#version: 0.2`
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            let mut it = line.split(' ');
            if let (Some(a), Some(b)) = (it.next(), it.next()) {
                ranks.insert((a.to_string(), b.to_string()), i);
            }
        }
        if vocab.is_empty() || ranks.is_empty() {
            return None;
        }
        Some(Bpe { vocab, ranks, byte_map: bytes_to_unicode() })
    }

    /// 编码成 token id，首尾加 `<s>` / `</s>`，右侧补 `<pad>`(=1)。
    pub fn encode(&self, text: &str, max_len: usize) -> Vec<i64> {
        let mut ids = Vec::with_capacity(max_len);
        ids.push(self.id("<s>").unwrap_or(0));
        for piece in pre_tokenize(text) {
            let mut syms: Vec<String> = piece
                .as_bytes()
                .iter()
                .map(|b| self.byte_map[*b as usize].to_string())
                .collect();
            self.merge(&mut syms);
            for s in syms {
                if ids.len() + 1 >= max_len {
                    break;
                }
                ids.push(self.id(&s).unwrap_or_else(|| self.id("<unk>").unwrap_or(3)));
            }
            if ids.len() + 1 >= max_len {
                break;
            }
        }
        ids.push(self.id("</s>").unwrap_or(2));
        ids.resize(max_len, self.id("<pad>").unwrap_or(1));
        ids
    }

    fn id(&self, s: &str) -> Option<i64> {
        self.vocab.get(s).copied()
    }

    /// 贪心 BPE：每轮合并 rank 最小的一对，合并一次后重新扫描。
    ///
    /// 必须与 GPT-2 的参考实现（以及 `tokenizers` 库）逐轮一致：一次只合并
    /// 一个最优对再重来。若在同一位置连续合并，长词会被合出参考实现里没有的
    /// 中间 token，id 错位后文本向量就整体跑偏（实测 CLAP 余弦从 0.20 掉到 0.13）。
    fn merge(&self, syms: &mut Vec<String>) {
        while syms.len() > 1 {
            let mut best: Option<(usize, usize)> = None;
            for i in 0..syms.len() - 1 {
                if let Some(&r) = self.ranks.get(&(syms[i].clone(), syms[i + 1].clone())) {
                    if best.map_or(true, |(_, br)| r < br) {
                        best = Some((i, r));
                    }
                }
            }
            let Some((i, _)) = best else { break };
            let merged = format!("{}{}", syms[i], syms[i + 1]);
            syms.remove(i + 1);
            syms[i] = merged;
        }
    }
}

/// 字符类别：GPT-2 预切分把字母、数字、其它符号分开处理。
fn class_of(c: char) -> u8 {
    if c.is_alphabetic() {
        0
    } else if c.is_numeric() {
        1
    } else {
        2
    }
}

/// GPT-2 的预切分规则（简化版）。
///
/// 原文是正则 `'s|'t|...| ?\p{L}+| ?\p{N}+| ?[^\s\p{L}\p{N}]+|\s+(?!\S)|\s+`，
/// 这里手写等价逻辑，省一个 regex 依赖。
///
/// **空白必须跟着它后面的词一起切**（` ?\p{L}+` 的语义）：`"a dog"` 要切成
/// `["a", " dog"]`，而不是 `["a", " ", "dog"]`。切成独立空格的后果是
/// `' dog'` 变成 `'Ġ' + 'dog'` 两个 token，而参考实现是 `'Ġdog'` 一个 token，
/// 多出来的 `Ġ` 会把 CLAP 的文本向量带偏。
fn pre_tokenize(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            let mut j = i;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            if j < chars.len() {
                // ` ?\p{L}+`：只保留紧邻词的**最后一个**空白
                let start = j - 1;
                let class = class_of(chars[j]);
                let mut k = j;
                while k < chars.len() && !chars[k].is_whitespace() && class_of(chars[k]) == class {
                    k += 1;
                }
                out.push(chars[start..k].iter().collect());
                i = k;
            } else {
                out.push(chars[i..j].iter().collect());
                i = j;
            }
            continue;
        }
        let class = class_of(c);
        let mut j = i;
        while j < chars.len() && !chars[j].is_whitespace() && class_of(chars[j]) == class {
            j += 1;
        }
        out.push(chars[i..j].iter().collect());
        i = j;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pre_token_splits_words() {
        let t = pre_tokenize("a dog barking");
        assert_eq!(t, vec!["a", " dog", " barking"]);
    }

    #[test]
    fn pre_token_splits_punct() {
        let t = pre_tokenize("hello, world!");
        assert_eq!(t, vec!["hello", ",", " world", "!"]);
    }
}

/// 诊断用：打印一句英文的 token id（对照 Python 参考实现）。
pub fn debug_tokens(text: &str, max_len: usize) -> Vec<i64> {
    let dir = std::env::var("CLAP_DIR").unwrap_or_else(|_| ".".to_string());
    match Bpe::load(Path::new(&dir)) {
        Some(b) => b.encode(text, max_len),
        None => vec![],
    }
}
