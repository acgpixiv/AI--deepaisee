//! SQLite 元数据层：文件表 + FTS5 全文索引 + 规则/分类/日志。
use parking_lot::Mutex;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const FTS_SCHEMA_VERSION: i64 = 3;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileRow {
    pub id: i64,
    pub path: String,
    pub dir: String,
    pub name: String,
    pub ext: String,
    pub kind: String,
    pub size: i64,
    pub mtime: i64,
    pub ctime: i64,
    pub text_len: i64,
    pub text_state: String,
    pub embed_state: String,
    pub thumb_key: Option<String>,
    pub category: Option<String>,
    pub tags: Option<String>,
}

impl FileRow {
    pub fn new(path: &str) -> Self {
        // 统一分隔符：用户可能粘贴 C:/a/b，walkdir 给的是 C:\a\b。
        // 不统一的话同一文件会在库里存成两条（path 是 UNIQUE，但字符串不同就绕过约束），
        // 表现为「统计 35 个文件，实际只有 16 个」。
        let norm = crate::util::norm_path(path);
        let p = Path::new(&norm);
        Self {
            id: 0,
            path: norm.clone(),
            dir: p.parent().map(|d| d.to_string_lossy().to_string()).unwrap_or_default(),
            name: p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
            ext: p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default(),
            kind: "other".into(),
            size: 0,
            mtime: 0,
            ctime: 0,
            text_len: 0,
            text_state: "pending".into(),
            embed_state: "pending".into(),
            thumb_key: None,
            category: None,
            tags: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    pub id: i64,
    pub name: String,
    pub field: String,
    pub op: String,
    pub value: String,
    pub category: String,
    pub priority: i64,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Activity {
    pub id: i64,
    pub ts: i64,
    pub kind: String,
    pub msg: String,
}

pub struct Db {
    conn: Mutex<Connection>,
}

impl Db {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        if let Some(d) = path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "cache_size", -32768)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let db = Self { conn: Mutex::new(conn) };
        db.migrate()?;
        Ok(db)
    }

    pub fn migrate(&self) -> rusqlite::Result<()> {
        let c = self.conn.lock();
        c.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS files(
                id INTEGER PRIMARY KEY,
                path TEXT NOT NULL UNIQUE,
                dir TEXT NOT NULL,
                name TEXT NOT NULL,
                ext TEXT NOT NULL,
                kind TEXT NOT NULL,
                size INTEGER NOT NULL DEFAULT 0,
                mtime INTEGER NOT NULL DEFAULT 0,
                ctime INTEGER NOT NULL DEFAULT 0,
                text_len INTEGER NOT NULL DEFAULT 0,
                text_state TEXT NOT NULL DEFAULT 'pending',
                embed_state TEXT NOT NULL DEFAULT 'pending',
                thumb_key TEXT,
                category TEXT,
                tags TEXT,
                lsh TEXT,
                missing INTEGER NOT NULL DEFAULT 0
            );
            CREATE INDEX IF NOT EXISTS ix_files_dir ON files(dir);
            CREATE INDEX IF NOT EXISTS ix_files_kind ON files(kind);
            CREATE INDEX IF NOT EXISTS ix_files_mtime ON files(mtime DESC);
            CREATE INDEX IF NOT EXISTS ix_files_size ON files(size DESC);
            CREATE INDEX IF NOT EXISTS ix_files_text ON files(text_state);
            CREATE INDEX IF NOT EXISTS ix_files_embed ON files(embed_state);
            CREATE INDEX IF NOT EXISTS ix_files_cat ON files(category);
            CREATE INDEX IF NOT EXISTS ix_files_missing ON files(missing);

            CREATE TABLE IF NOT EXISTS meta(k TEXT PRIMARY KEY, v TEXT);
            CREATE TABLE IF NOT EXISTS rules(
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                field TEXT NOT NULL,
                op TEXT NOT NULL,
                value TEXT NOT NULL,
                category TEXT NOT NULL,
                priority INTEGER NOT NULL DEFAULT 100,
                enabled INTEGER NOT NULL DEFAULT 1
            );
            CREATE TABLE IF NOT EXISTS activity(
                id INTEGER PRIMARY KEY,
                ts INTEGER NOT NULL,
                kind TEXT NOT NULL,
                msg TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS ix_activity_ts ON activity(ts DESC);
            "#,
        )?;
        // 历史遗留：早期版本按原始字符串存 path，用户粘贴 C:/a/b 与 walkdir 的 C:\a\b
        // 会被当成两个文件。这里把正斜杠统一成反斜杠；若统一后撞了 UNIQUE，
        // 说明是重复行，直接删掉后来者（保留 id 小的、状态更完整的）。
        c.execute(
            "UPDATE OR IGNORE files SET path = replace(path, '/', '\\') \
             WHERE instr(path, '/') > 0",
            [],
        )?;
        c.execute(
            "DELETE FROM files WHERE id NOT IN \
             (SELECT MIN(id) FROM files GROUP BY replace(path, '/', '\\'))",
            [],
        )?;
        // FTS5：unicode61 + 应用层中文 2-gram。
        //
        // docs.rowid 直接等于 files.id，一条文件一行正文，天然一一对应。
        // 早期版本用 docs_map 间接映射，但 FTS5 虚表的 last_insert_rowid()
        // 不可靠，加上文件删除后 id 会被复用，映射会指到别人的正文上 ——
        // 表现为「搜到 A 文件，高亮片段却是 B 的内容」。这里直接砍掉映射表。
        let existing: Option<String> = c
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='docs'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        let prev_ver: i64 = c
            .query_row("SELECT v FROM meta WHERE k='schema_version'", [], |r| {
                r.get::<_, String>(0)
            })
            .optional()?
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let need_rebuild = match &existing {
            None => true,
            Some(sql) => {
                prev_ver < FTS_SCHEMA_VERSION
                    || !sql.to_lowercase().contains("unicode61")
                    || !sql.contains("raw")
            }
        };
        if need_rebuild {
            c.execute_batch("DROP TABLE IF EXISTS docs; DROP TABLE IF EXISTS docs_map;")?;
            // 顺带把 skip 也重置：旧版本因为 "doc" 判定 bug，把整类文档都标成了
            // skip，光重建表不清这个状态，那些文件仍然永远不会被重新抽取。
            c.execute(
                "UPDATE files SET text_state='pending' WHERE text_state IN ('ok','skip')",
                [],
            )?;
            c.execute_batch(
                "CREATE VIRTUAL TABLE docs USING fts5(name, body, raw UNINDEXED, tokenize='unicode61');",
            )?;
        } else {
            // 文件删掉后正文还留着 = 孤儿，会命中已不存在的文件，顺手清掉。
            c.execute("DELETE FROM docs WHERE rowid NOT IN (SELECT id FROM files)", [])?;
        }
        c.execute(
            "INSERT INTO meta(k,v) VALUES('schema_version',?1) ON CONFLICT(k) DO UPDATE SET v=excluded.v",
            params![FTS_SCHEMA_VERSION.to_string()],
        )?;
        Ok(())
    }

    // ---------------- 文件表
    pub fn upsert_file(&self, f: &FileRow) -> rusqlite::Result<i64> {
        let c = self.conn.lock();
        c.execute(
            r#"INSERT INTO files(path,dir,name,ext,kind,size,mtime,ctime,text_state,embed_state,category,tags,missing)
               VALUES(?1,?2,?3,?4,?5,?6,?7,?8,'pending','pending',?9,?10,0)
               ON CONFLICT(path) DO UPDATE SET
                 dir=excluded.dir, name=excluded.name, ext=excluded.ext, kind=excluded.kind,
                 size=excluded.size, mtime=excluded.mtime, ctime=excluded.ctime, missing=0"#,
            params![f.path, f.dir, f.name, f.ext, f.kind, f.size, f.mtime, f.ctime, f.category, f.tags],
        )?;
        Ok(c.last_insert_rowid())
    }

    pub fn get_by_path(&self, path: &str) -> rusqlite::Result<Option<FileRow>> {
        let c = self.conn.lock();
        c.query_row(
            "SELECT id,path,dir,name,ext,kind,size,mtime,ctime,text_len,text_state,embed_state,thumb_key,category,tags
             FROM files WHERE path=?1",
            params![path],
            row_to_file,
        )
        .optional()
    }

    pub fn get_by_id(&self, id: i64) -> rusqlite::Result<Option<FileRow>> {
        let c = self.conn.lock();
        c.query_row(
            "SELECT id,path,dir,name,ext,kind,size,mtime,ctime,text_len,text_state,embed_state,thumb_key,category,tags
             FROM files WHERE id=?1",
            params![id],
            row_to_file,
        )
        .optional()
    }

    pub fn count(&self) -> rusqlite::Result<i64> {
        let c = self.conn.lock();
        c.query_row("SELECT COUNT(*) FROM files WHERE missing=0", [], |r| r.get(0))
    }

    pub fn total_bytes(&self) -> rusqlite::Result<i64> {
        let c = self.conn.lock();
        c.query_row("SELECT COALESCE(SUM(size),0) FROM files WHERE missing=0", [], |r| r.get(0))
    }

    /// 各类别统计：kind -> (数量, 总大小)
    pub fn kind_stats(&self) -> rusqlite::Result<Vec<(String, i64, i64)>> {
        let c = self.conn.lock();
        let mut st = c.prepare(
            "SELECT kind, COUNT(*), COALESCE(SUM(size),0) FROM files WHERE missing=0 GROUP BY kind ORDER BY 2 DESC",
        )?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        rows.collect()
    }

    pub fn update_fields(&self, old_path: &str, fields: &[(&str, String)]) -> rusqlite::Result<()> {
        // 白名单列名，避免 SQL 注入
        const ALLOWED: &[&str] = &[
            "path", "dir", "name", "ext", "kind", "size", "mtime", "ctime", "text_len",
            "text_state", "embed_state", "thumb_key", "category", "tags", "missing",
        ];
        let mut sets = Vec::new();
        let mut vals: Vec<rusqlite::types::Value> = Vec::new();
        for (k, v) in fields {
            if !ALLOWED.contains(k) {
                continue;
            }
            sets.push(format!("{k}=?"));
            vals.push(v.clone().into());
        }
        if sets.is_empty() {
            return Ok(());
        }
        let sql = format!("UPDATE files SET {} WHERE path=?", sets.join(","));
        vals.push(old_path.to_string().into());
        let c = self.conn.lock();
        c.execute(&sql, rusqlite::params_from_iter(vals))?;
        Ok(())
    }

    pub fn mark_missing(&self, path: &str) -> rusqlite::Result<()> {
        let c = self.conn.lock();
        c.execute("UPDATE files SET missing=1 WHERE path=?1", params![path])?;
        Ok(())
    }

    /// 整个根目录消失时，把它下面所有记录一次性标成 missing。
    pub fn mark_missing_prefix(&self, prefix: &str) -> rusqlite::Result<usize> {
        let c = self.conn.lock();
        let n = c.execute(
            "UPDATE files SET missing=1 WHERE missing=0 AND (path LIKE ?1 || '%')",
            params![prefix],
        )?;
        Ok(n)
    }

    /// 把某个扫描根下、本次扫描**没遇到**的记录标成 missing，返回受影响行数。
    ///
    /// 只在扫描完整跑完（未取消）时调用 —— 取消时还没走到的目录不该被误判成已删除。
    /// 用临时表而不是 `NOT IN (...)`：几十万 id 拼进 SQL 会超长，而且慢。
    pub fn mark_missing_except(
        &self,
        prefix: &str,
        seen: &std::collections::HashSet<i64>,
    ) -> rusqlite::Result<usize> {
        let mut c = self.conn.lock();
        let tx = c.transaction()?;
        tx.execute("CREATE TEMP TABLE IF NOT EXISTS _scan_seen(id INTEGER PRIMARY KEY)", [])?;
        tx.execute("DELETE FROM _scan_seen", [])?;
        {
            let mut st = tx.prepare("INSERT OR IGNORE INTO _scan_seen(id) VALUES(?1)")?;
            for id in seen {
                st.execute(params![id])?;
            }
        }
        let n = tx.execute(
            "UPDATE files SET missing=1 \
             WHERE missing=0 AND path LIKE ?1 || '%' \
               AND id NOT IN (SELECT id FROM _scan_seen)",
            params![prefix],
        )?;
        tx.execute("DROP TABLE IF EXISTS _scan_seen", [])?;
        tx.commit()?;
        Ok(n)
    }

    pub fn delete_ids(&self, ids: &[i64]) -> rusqlite::Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let mut c = self.conn.lock();
        let tx = c.transaction()?;
        for id in ids {
            tx.execute("DELETE FROM docs WHERE rowid=?1", params![id])?;
            tx.execute("DELETE FROM files WHERE id=?1", params![id])?;
        }
        tx.commit()?;
        Ok(())
    }

    // ---------------- 全文索引
    /// 写入正文。docs.rowid 恒等于 files.id，重复写入即整行替换。
    pub fn set_doc(&self, file_id: i64, name: &str, body: &str, raw: &str) -> rusqlite::Result<()> {
        let c = self.conn.lock();
        // FTS5 虚表对 INSERT OR REPLACE 的支持依赖 conflict 支持，直接
        // 先删后插最稳，也顺便让 rowid 不会漂移。
        c.execute("DELETE FROM docs WHERE rowid=?1", params![file_id])?;
        c.execute(
            "INSERT INTO docs(rowid, name, body, raw) VALUES(?1,?2,?3,?4)",
            params![file_id, name, body, raw],
        )?;
        c.execute(
            "UPDATE files SET text_len=?1, text_state='ok' WHERE id=?2",
            params![raw.chars().count() as i64, file_id],
        )?;
        Ok(())
    }

    pub fn get_doc_raw(&self, file_id: i64) -> rusqlite::Result<Option<String>> {
        let c = self.conn.lock();
        c.query_row("SELECT raw FROM docs WHERE rowid=?1", params![file_id], |r| r.get(0))
            .optional()
    }

    /// FTS 检索，返回 (file_id, bm25 分数)，分数越小越相关，这里转成升序。
    ///
    /// 连接 files 表是为了挡掉 missing=1：文件在磁盘上删了、扫描后只被标了
    /// missing，正文还留在 docs 里，不挡的话它会一直出现在全文结果里。
    /// 注意不能给 docs 起别名，FTS5 的 MATCH 认的是表名。
    pub fn search_fts(&self, expr: &str, limit: usize) -> rusqlite::Result<Vec<(i64, f64)>> {
        let c = self.conn.lock();
        let mut st = c.prepare(
            "SELECT docs.rowid, bm25(docs,2.0,1.0,0.0) FROM docs
             JOIN files f ON f.id=docs.rowid AND f.missing=0
             WHERE docs MATCH ?1 ORDER BY 2 LIMIT ?2",
        )?;
        let rows = st.query_map(params![expr, limit as i64], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect()
    }

    pub fn set_state(&self, file_id: i64, what: &str, value: &str) -> rusqlite::Result<()> {
        let col = match what {
            "text" => "text_state",
            "embed" => "embed_state",
            _ => return Ok(()),
        };
        let c = self.conn.lock();
        c.execute(&format!("UPDATE files SET {col}=?1 WHERE id=?2"), params![value, file_id])?;
        Ok(())
    }

    /// 把全部（未缺失）文件的状态标回 `pending`，用于强制重建。
    /// 换嵌入模型后向量维度会变、旧向量被清空，但 embed_state 仍是 ok，
    /// 不重置的话「重建索引」会排队 0 个文件，用户以为功能坏了。
    pub fn reset_states(&self, what: &str) -> rusqlite::Result<usize> {
        let cols: &[&str] = match what {
            "text" => &["text_state"],
            "embed" => &["embed_state"],
            _ => &["text_state", "embed_state"],
        };
        let c = self.conn.lock();
        let mut n = 0;
        for col in cols {
            n = c.execute(
                &format!("UPDATE files SET {col}='pending' WHERE missing=0"),
                [],
            )?;
        }
        Ok(n)
    }

    pub fn set_thumb(&self, file_id: i64, key: &str) -> rusqlite::Result<()> {
        let c = self.conn.lock();
        c.execute("UPDATE files SET thumb_key=?1 WHERE id=?2", params![key, file_id])?;
        Ok(())
    }

    pub fn pending_ids(&self, what: &str, limit: usize) -> rusqlite::Result<Vec<i64>> {
        // "auto" 必须同时看两列：只看 embed_state 的话，一个文件正文没抽、
        // 但向量已经建好，它就永远不会被排进全文队列 —— 这正是修复
        // `kind=="doc"` 那个判定 bug 之后文章仍然搜不到的原因。
        let cond = match what {
            "text" => "text_state IN ('pending','fail')",
            "embed" => "embed_state IN ('pending','fail')",
            _ => "text_state IN ('pending','fail') OR embed_state IN ('pending','fail')",
        };
        let c = self.conn.lock();
        let mut st = c.prepare(&format!(
            "SELECT id FROM files WHERE missing=0 AND ({cond}) LIMIT ?1"
        ))?;
        let rows = st.query_map(params![limit as i64], |r| r.get(0))?;
        rows.collect()
    }

    pub fn count_state(&self, what: &str, state: &str) -> rusqlite::Result<i64> {
        let col = if what == "text" { "text_state" } else { "embed_state" };
        let c = self.conn.lock();
        c.query_row(
            &format!("SELECT COUNT(*) FROM files WHERE missing=0 AND {col}=?1"),
            params![state],
            |r| r.get(0),
        )
    }

    // ---------------- 规则 / 分类 / 日志
    pub fn list_rules(&self) -> rusqlite::Result<Vec<Rule>> {
        let c = self.conn.lock();
        let mut st = c.prepare(
            "SELECT id,name,field,op,value,category,priority,enabled FROM rules ORDER BY priority,id",
        )?;
        let rows = st.query_map([], |r| {
            Ok(Rule {
                id: r.get(0)?,
                name: r.get(1)?,
                field: r.get(2)?,
                op: r.get(3)?,
                value: r.get(4)?,
                category: r.get(5)?,
                priority: r.get(6)?,
                enabled: r.get::<_, i64>(7)? != 0,
            })
        })?;
        rows.collect()
    }

    pub fn replace_rules(&self, rules: &[Rule]) -> rusqlite::Result<()> {
        let mut c = self.conn.lock();
        let tx = c.transaction()?;
        tx.execute("DELETE FROM rules", [])?;
        for r in rules {
            tx.execute(
                "INSERT INTO rules(name,field,op,value,category,priority,enabled) VALUES(?1,?2,?3,?4,?5,?6,?7)",
                params![r.name, r.field, r.op, r.value, r.category, r.priority, if r.enabled { 1 } else { 0 }],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn log(&self, kind: &str, msg: &str) -> rusqlite::Result<()> {
        let c = self.conn.lock();
        let ts = crate::util::now_ts();
        c.execute(
            "INSERT INTO activity(ts,kind,msg) VALUES(?1,?2,?3)",
            params![ts, kind, msg],
        )?;
        Ok(())
    }

    pub fn recent_activity(&self, limit: usize, kind: Option<&str>) -> rusqlite::Result<Vec<Activity>> {
        let c = self.conn.lock();
        let mut out = Vec::new();
        match kind {
            Some(k) => {
                let mut st = c.prepare(
                    "SELECT id,ts,kind,msg FROM activity WHERE kind=?1 ORDER BY id DESC LIMIT ?2",
                )?;
                let rows = st.query_map(params![k, limit as i64], |r| {
                    Ok(Activity { id: r.get(0)?, ts: r.get(1)?, kind: r.get(2)?, msg: r.get(3)? })
                })?;
                for r in rows {
                    out.push(r?);
                }
            }
            None => {
                let mut st =
                    c.prepare("SELECT id,ts,kind,msg FROM activity ORDER BY id DESC LIMIT ?1")?;
                let rows = st.query_map(params![limit as i64], |r| {
                    Ok(Activity { id: r.get(0)?, ts: r.get(1)?, kind: r.get(2)?, msg: r.get(3)? })
                })?;
                for r in rows {
                    out.push(r?);
                }
            }
        }
        Ok(out)
    }

    pub fn clear_all(&self) -> rusqlite::Result<()> {
        let c = self.conn.lock();
        c.execute_batch(
            "DELETE FROM files; DELETE FROM docs; DELETE FROM activity;",
        )?;
        Ok(())
    }

    pub fn vacuum(&self) -> rusqlite::Result<()> {
        let c = self.conn.lock();
        c.execute_batch("VACUUM;")?;
        Ok(())
    }

    /// 通用查询：返回 FileRow 列表（SQL 由调用方拼装，参数用占位符传入）。
    pub fn query_rows(&self, sql: &str, args: Vec<rusqlite::types::Value>) -> Vec<FileRow> {
        let c = self.conn.lock();
        let mut st = match c.prepare(sql) {
            Ok(s) => s,
            Err(_) => return vec![],
        };
        let mut rows = match st.query(rusqlite::params_from_iter(args)) {
            Ok(r) => r,
            Err(_) => return vec![],
        };
        let mut out = Vec::new();
        while let Ok(Some(row)) = rows.next() {
            if let Ok(f) = row_to_file(row) {
                out.push(f);
            }
        }
        out
    }

    /// 目录聚合：返回 (dir, 文件数, 总大小)
    pub fn dir_rollup(&self) -> rusqlite::Result<Vec<(String, i64, i64)>> {
        let c = self.conn.lock();
        let mut st = c.prepare(
            "SELECT dir, COUNT(*), COALESCE(SUM(size),0) FROM files WHERE missing=0 GROUP BY dir",
        )?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        rows.collect()
    }
}

fn row_to_file(r: &rusqlite::Row<'_>) -> rusqlite::Result<FileRow> {
    Ok(FileRow {
        id: r.get(0)?,
        path: r.get(1)?,
        dir: r.get(2)?,
        name: r.get(3)?,
        ext: r.get(4)?,
        kind: r.get(5)?,
        size: r.get(6)?,
        mtime: r.get(7)?,
        ctime: r.get(8)?,
        text_len: r.get(9)?,
        text_state: r.get(10)?,
        embed_state: r.get(11)?,
        thumb_key: r.get(12)?,
        category: r.get(13)?,
        tags: r.get(14)?,
    })
}
