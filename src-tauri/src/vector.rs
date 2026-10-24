//! 向量存储：文件 id -> f32 向量，落盘为二进制并常驻内存。
use parking_lot::RwLock;
use std::collections::HashMap;
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};

pub struct VectorStore {
    dim: usize,
    dir: PathBuf,
    data: RwLock<Vec<f32>>,      // slot * dim
    ids: RwLock<Vec<i64>>,       // slot -> file_id（-1 表示空槽）
    index: RwLock<HashMap<i64, usize>>, // file_id -> slot
}

impl VectorStore {
    pub fn open(dir: &Path, dim: usize) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let store = Self {
            dim,
            dir: dir.to_path_buf(),
            data: RwLock::new(Vec::new()),
            ids: RwLock::new(Vec::new()),
            index: RwLock::new(HashMap::new()),
        };
        store.load()?;
        Ok(store)
    }

    pub fn dim(&self) -> usize {
        self.dim
    }

    pub fn len(&self) -> usize {
        self.index.read().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn vec_path(&self) -> PathBuf {
        self.dir.join("vectors.bin")
    }
    fn id_path(&self) -> PathBuf {
        self.dir.join("ids.bin")
    }
    fn meta_path(&self) -> PathBuf {
        self.dir.join("meta.json")
    }

    /// 读维度元数据。老版本没有 meta.json 时返回 None。
    fn saved_dim(&self) -> Option<usize> {
        let s = std::fs::read_to_string(self.meta_path()).ok()?;
        let v: serde_json::Value = serde_json::from_str(&s).ok()?;
        v.get("dim")?.as_u64().map(|d| d as usize)
    }

    fn write_meta(&self) -> std::io::Result<()> {
        let meta = serde_json::json!({ "dim": self.dim, "count": self.len() });
        std::fs::write(self.meta_path(), meta.to_string())
    }

    fn load(&self) -> std::io::Result<()> {
        let vp = self.vec_path();
        let ip = self.id_path();
        if !vp.exists() || !ip.exists() {
            return Ok(());
        }
        // 维度不一致说明换过 embedding 后端（如 hash 256 → llama 512），
        // 旧向量全部作废，直接清空重建，否则数据会按错误步长错位。
        if let Some(d) = self.saved_dim() {
            if d != self.dim {
                let _ = std::fs::remove_file(&vp);
                let _ = std::fs::remove_file(&ip);
                let _ = self.write_meta();
                return Ok(());
            }
        }
        let mut buf = Vec::new();
        std::fs::File::open(&vp)?.read_to_end(&mut buf)?;
        let data: Vec<f32> = buf
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        let mut ibuf = Vec::new();
        std::fs::File::open(&ip)?.read_to_end(&mut ibuf)?;
        let ids: Vec<i64> = ibuf
            .chunks_exact(8)
            .map(|c| i64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]))
            .collect();
        if self.dim > 0 && data.len() != ids.len() * self.dim {
            // 文件损坏或维度记录缺失：保守起见清空，避免越界
            let _ = std::fs::remove_file(&vp);
            let _ = std::fs::remove_file(&ip);
            let _ = self.write_meta();
            return Ok(());
        }
        let slots = ids.len();
        let mut index = HashMap::with_capacity(slots);
        for (slot, &fid) in ids.iter().enumerate() {
            if fid >= 0 {
                index.insert(fid, slot);
            }
        }
        *self.data.write() = data;
        *self.ids.write() = ids;
        *self.index.write() = index;
        Ok(())
    }

    pub fn save(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let _ = self.write_meta();
        // 原子替换：先写临时文件再 rename（Windows 上 rename 覆盖需要先删）
        let tmp = self.dir.join("vectors.bin.tmp");
        {
            let data = self.data.read();
            let mut w = BufWriter::new(std::fs::File::create(&tmp)?);
            let mut bytes = Vec::with_capacity(data.len() * 4);
            for v in data.iter() {
                bytes.extend_from_slice(&v.to_le_bytes());
            }
            w.write_all(&bytes)?;
            w.flush()?;
        }
        let tmp_id = self.dir.join("ids.bin.tmp");
        {
            let ids = self.ids.read();
            let mut w = BufWriter::new(std::fs::File::create(&tmp_id)?);
            let mut bytes = Vec::with_capacity(ids.len() * 8);
            for v in ids.iter() {
                bytes.extend_from_slice(&v.to_le_bytes());
            }
            w.write_all(&bytes)?;
            w.flush()?;
        }
        replace(&tmp, &self.vec_path())?;
        replace(&tmp_id, &self.id_path())?;
        Ok(())
    }

    pub fn add(&self, file_id: i64, vec: &[f32]) {
        debug_assert_eq!(vec.len(), self.dim);
        let mut index = self.index.write();
        let mut data = self.data.write();
        let mut ids = self.ids.write();
        if let Some(&slot) = index.get(&file_id) {
            data[slot * self.dim..(slot + 1) * self.dim].copy_from_slice(vec);
            return;
        }
        let slot = ids.len();
        ids.push(file_id);
        data.extend_from_slice(vec);
        index.insert(file_id, slot);
    }

    pub fn has(&self, file_id: i64) -> bool {
        self.index.read().contains_key(&file_id)
    }

    pub fn get(&self, file_id: i64) -> Option<Vec<f32>> {
        let index = self.index.read();
        let slot = *index.get(&file_id)?;
        let data = self.data.read();
        Some(data[slot * self.dim..(slot + 1) * self.dim].to_vec())
    }

    pub fn remove(&self, file_id: i64) {
        let mut index = self.index.write();
        if let Some(slot) = index.remove(&file_id) {
            let mut ids = self.ids.write();
            ids[slot] = -1;
        }
    }

    /// 余弦相似检索（向量均已归一化，直接点积）。
    pub fn search(&self, query: &[f32], top_k: usize, allowed: Option<&std::collections::HashSet<i64>>) -> Vec<(i64, f32)> {
        let index = self.index.read();
        let data = self.data.read();
        let mut scored: Vec<(i64, f32)> = Vec::with_capacity(index.len());
        for (&fid, &slot) in index.iter() {
            if let Some(a) = allowed {
                if !a.contains(&fid) {
                    continue;
                }
            }
            let base = slot * self.dim;
            let mut dot = 0.0f32;
            for i in 0..self.dim {
                dot += data[base + i] * query[i];
            }
            scored.push((fid, dot));
        }
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);
        scored
    }

    pub fn compact(&self) -> std::io::Result<()> {
        let mut index = self.index.write();
        let mut ids = self.ids.write();
        let mut data = self.data.write();
        let mut new_ids = Vec::with_capacity(index.len());
        let mut new_data = Vec::with_capacity(index.len() * self.dim);
        let mut new_index = HashMap::with_capacity(index.len());
        for (slot, &fid) in ids.iter().enumerate() {
            if fid >= 0 {
                new_index.insert(fid, new_ids.len());
                new_ids.push(fid);
                new_data.extend_from_slice(&data[slot * self.dim..(slot + 1) * self.dim]);
            }
        }
        *ids = new_ids;
        *data = new_data;
        *index = new_index;
        drop(index);
        drop(ids);
        drop(data);
        self.save()
    }

    pub fn clear(&self) -> std::io::Result<()> {
        *self.data.write() = Vec::new();
        *self.ids.write() = Vec::new();
        *self.index.write() = HashMap::new();
        let _ = std::fs::remove_file(self.vec_path());
        let _ = std::fs::remove_file(self.id_path());
        Ok(())
    }
}

fn replace(tmp: &Path, dst: &Path) -> std::io::Result<()> {
    if dst.exists() {
        // Windows 上无法直接 rename 覆盖已有文件
        std::fs::remove_file(dst)?;
    }
    std::fs::rename(tmp, dst)
}

/// 归一化（原地）。
pub fn normalize(v: &mut [f32]) {
    let mut sum = 0.0f64;
    for x in v.iter() {
        sum += (*x as f64) * (*x as f64);
    }
    let n = sum.sqrt();
    if n > 1e-8 {
        let inv = (1.0 / n) as f32;
        for x in v.iter_mut() {
            *x *= inv;
        }
    }
}

pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let mut dot = 0.0f32;
    for i in 0..a.len().min(b.len()) {
        dot += a[i] * b[i];
    }
    dot
}
