<script setup lang="ts">
import { computed } from "vue";
import { Database, Plus, RefreshCw, Save, Trash2 } from "lucide-vue-next";
import type { AppConfig, Rule } from "../api";
import ScanView from "../views/ScanView.vue";

const props = defineProps<{
  cfg: AppConfig;
  rules: Rule[];
  newRule: Rule;
  excludeDirsText: string;
  dataDir: string;
  saveFileConfig: () => Promise<boolean | void>;
  addRule: () => Promise<void>;
  deleteRule: (index: number) => Promise<void>;
  applyRules: () => Promise<void>;
  saveRules: () => Promise<void>;
  vacuum: () => Promise<void>;
  clearAll: () => Promise<void>;
}>();

const emit = defineEmits<{
  (event: "update-exclude-dirs", value: string): void;
}>();

const excludeDirs = computed({
  get: () => props.excludeDirsText,
  set: (value: string) => emit("update-exclude-dirs", value),
});
</script>

<template>
  <div class="settings-index-tab">
    <div class="embedded-view scan-embed"><ScanView /></div>

    <section v-if="false" class="settings-section common-settings">
      <div class="section-head">
        <div>
          <h2>通用设置</h2>
          <p>搜索时可选择目标语言。多语言嵌入模型会直接进行跨语言匹配。</p>
        </div>
        <button class="save-button" @click="props.saveFileConfig"><Save :size="14" />保存</button>
      </div>
      <div class="language-options">
        <span class="language-label">搜索目标语言</span>
        <label v-for="item in [
          { value: 'en', label: '英语 / English' },
          { value: 'ja', label: '日语 / 日本語' },
          { value: 'de', label: '德语 / Deutsch' },
          { value: 'fr', label: '法语 / Français' },
          { value: 'es', label: '西班牙语 / Español' },
          { value: 'ko', label: '韩语 / 한국어' }
        ]" :key="item.value" class="language-option">
          <span></span>
          <span>{{ item.label }}</span>
        </label>
      </div>
    </section>

    <section class="settings-section">
      <div class="section-head">
        <div>
          <h2>索引参数</h2>
          <p>控制正文、缩略图和向量任务的处理范围。</p>
        </div>
        <button class="save-button" @click="props.saveFileConfig"><Save :size="14" />保存</button>
      </div>
      <div class="form-grid three">
        <label>最大文件（MB）<input type="number" v-model.number="props.cfg.scan.max_size_mb" /></label>
        <label>最小文件（KB）<input type="number" v-model.number="props.cfg.scan.min_size_kb" /></label>
        <label>正文最大字符数<input type="number" v-model.number="props.cfg.index.max_text_chars" /></label>
        <label>图片最长边<input type="number" v-model.number="props.cfg.index.image_max_side" /></label>
        <label>视频抽帧数<input type="number" v-model.number="props.cfg.index.video_frames" /></label>
        <label>音频秒数<input type="number" step="0.5" v-model.number="props.cfg.index.audio_seconds" /></label>
        <label>索引批量大小<input type="number" v-model.number="props.cfg.index.batch_size" /></label>
        <label>搜索结果数<input type="number" v-model.number="props.cfg.search.page_size" /></label>
        <label>语义权重<input type="number" min="0" max="1" step="0.05" v-model.number="props.cfg.search.semantic_weight" /></label>
        <label>语义最低分<input type="number" min="0" max="1" step="0.05" v-model.number="props.cfg.search.semantic_floor" /></label>
        <label>输入防抖（毫秒）<input type="number" min="0" step="50" v-model.number="props.cfg.search.debounce_ms" /></label>
      </div>
      <div class="switch-row">
        <label><input type="checkbox" v-model="props.cfg.index.auto_fulltext" />自动抽取正文</label>
        <label><input type="checkbox" v-model="props.cfg.index.auto_embed" />自动生成向量</label>
        <label><input type="checkbox" v-model="props.cfg.search.use_pinyin" />启用拼音检索</label>
        <label><input type="checkbox" v-model="props.cfg.search.use_fuzzy" />启用模糊匹配</label>
        <label><input type="checkbox" v-model="props.cfg.scan.follow_links" />扫描时跟随符号链接</label>
      </div>
      <label class="wide-label exclude-label">
        排除目录（每行一个名称）
        <textarea v-model="excludeDirs" rows="3" placeholder="node_modules&#10;.git&#10;System Volume Information"></textarea>
      </label>
    </section>

    <section class="settings-section">
      <div class="section-head">
        <div>
          <h2>分类规则</h2>
          <p>按文件名、扩展名、路径或类型自动分类。</p>
        </div>
      </div>
      <div class="rule-form">
        <select v-model="props.newRule.field">
          <option value="name">文件名</option>
          <option value="ext">扩展名</option>
          <option value="path">完整路径</option>
          <option value="kind">文件类型</option>
        </select>
        <select v-model="props.newRule.op">
          <option value="contains">包含</option>
          <option value="equals">等于</option>
          <option value="startswith">开头是</option>
          <option value="endswith">结尾是</option>
          <option value="regex">正则</option>
        </select>
        <input v-model="props.newRule.value" placeholder="匹配内容，如 合同 / .jpg" />
        <input v-model="props.newRule.category" placeholder="归为分类" />
        <button class="save-button" @click="props.addRule"><Plus :size="14" />添加</button>
      </div>
      <div class="rule-list">
        <div v-for="(rule, index) in props.rules" :key="index" class="rule-row">
          <input type="checkbox" v-model="rule.enabled" />
          <span class="mono">{{ rule.field }} {{ rule.op }} "{{ rule.value }}"</span>
          <span class="muted">→ {{ rule.category }}</span>
          <button class="icon-button tiny-button" title="删除规则" aria-label="删除规则" @click="props.deleteRule(index)"><Trash2 :size="14" /></button>
        </div>
        <div v-if="!props.rules.length" class="empty-line">还没有规则</div>
      </div>
      <div class="action-row">
        <button class="save-button" @click="props.applyRules"><RefreshCw :size="14" />重新分类</button>
        <button class="secondary-button" @click="props.saveRules"><Save :size="14" />保存规则</button>
      </div>
    </section>

    <section class="settings-section data-section">
      <div><h2>索引数据库</h2><p class="muted">{{ props.dataDir }}</p></div>
      <div class="action-row">
        <button class="secondary-button" @click="props.vacuum"><Database :size="14" />压缩数据库</button>
        <button class="danger-button" @click="props.clearAll"><Trash2 :size="14" />清空索引</button>
      </div>
    </section>
  </div>
</template>

<style scoped>
</style>

