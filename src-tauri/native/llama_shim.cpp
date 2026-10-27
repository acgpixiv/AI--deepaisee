// llama.cpp 的极简 C ABI 包装层。
//
// 为什么需要它：llama.h 里的 llama_model_params / llama_context_params /
// llama_batch 都是按值传递的 struct，字段顺序随版本变化。在 Rust 里手写
// 这些 struct 的镜像，只要 llama.cpp 升一次版本就可能内存布局错位而崩溃。
// 这里用 C++ 直接包含 llama.h 编译，只向 Rust 暴露 5 个简单函数。

#include "llama.h"

#include <cmath>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <algorithm>
#include <mutex>
#include <string>
#include <thread>
#include <vector>

namespace {

std::once_flag backend_init_once;

void ensure_backend_initialized() {
    std::call_once(backend_init_once, [] {
        // Backend enumeration can itself print device discovery lines. The
        // desktop app reports device state through its own API, so keep the
        // native logger quiet before initialization as well as during model
        // execution.
        llama_log_set([](ggml_log_level, const char *, void *) {}, nullptr);
        llama_backend_init();
    });
}

struct Handle {
    llama_model *model = nullptr;
    llama_context *ctx = nullptr;
    const llama_vocab *vocab = nullptr;
    int dim = 0;      // 输出向量维度（可能是投影后的 768）
    int hidden = 0;   // 隐层维度（512），仅用于诊断
    int n_batch = 0;  // 单次 decode 能接受的最大 token 数（= cparams.n_batch）
};

void set_err(char *err, int err_len, const char *msg) {
    if (err && err_len > 0) {
        std::snprintf(err, static_cast<size_t>(err_len), "%s", msg ? msg : "unknown error");
    }
}

std::string device_key(ggml_backend_dev_t dev) {
    ggml_backend_dev_props props{};
    ggml_backend_dev_get_props(dev, &props);
    const char *backend = ggml_backend_reg_name(ggml_backend_dev_backend_reg(dev));
    const char *name = ggml_backend_dev_name(dev);
    std::string key = backend ? backend : "unknown";
    key.push_back('|');
    key += props.device_id && *props.device_id ? props.device_id : (name ? name : "unknown");
    return key;
}

}  // namespace

extern "C" {

// Enumerate devices from the exact GGML backends linked into this build. This
// deliberately excludes CPU/BLAS devices: they are not accelerators that can
// satisfy a user's GPU selection. CUDA and Vulkan builds expose only devices
// supported by their linked backends.
int ds_llama_device_count() {
    ensure_backend_initialized();
    int count = 0;
    for (size_t i = 0; i < ggml_backend_dev_count(); ++i) {
        const auto dev = ggml_backend_dev_get(i);
        const auto type = ggml_backend_dev_type(dev);
        if (type == GGML_BACKEND_DEVICE_TYPE_GPU || type == GGML_BACKEND_DEVICE_TYPE_IGPU) {
            ++count;
        }
    }
    return count;
}

int ds_llama_device_info(int wanted, char *name, int name_len, char *description,
                         int description_len, char *device_id, int device_id_len,
                         char *physical_id, int physical_id_len,
                         char *backend, int backend_len, std::uint64_t *free_bytes,
                         std::uint64_t *total_bytes, int *type) {
    ensure_backend_initialized();
    if (wanted < 0 || !name || name_len <= 0 || !description || description_len <= 0 ||
        !device_id || device_id_len <= 0 || !physical_id || physical_id_len <= 0 ||
        !backend || backend_len <= 0) {
        return -1;
    }
    int current = 0;
    for (size_t i = 0; i < ggml_backend_dev_count(); ++i) {
        const auto dev = ggml_backend_dev_get(i);
        const auto dev_type = ggml_backend_dev_type(dev);
        if (dev_type != GGML_BACKEND_DEVICE_TYPE_GPU && dev_type != GGML_BACKEND_DEVICE_TYPE_IGPU) {
            continue;
        }
        if (current++ != wanted) {
            continue;
        }
        const char *dev_name = ggml_backend_dev_name(dev);
        const char *dev_desc = ggml_backend_dev_description(dev);
        ggml_backend_dev_props props{};
        ggml_backend_dev_get_props(dev, &props);
        const char *reg_name = ggml_backend_reg_name(ggml_backend_dev_backend_reg(dev));
        std::snprintf(name, static_cast<size_t>(name_len), "%s", dev_name ? dev_name : "");
        std::snprintf(description, static_cast<size_t>(description_len), "%s", dev_desc ? dev_desc : "");
        const auto key = device_key(dev);
        std::snprintf(device_id, static_cast<size_t>(device_id_len), "%s", key.c_str());
        std::snprintf(physical_id, static_cast<size_t>(physical_id_len), "%s", props.device_id ? props.device_id : "");
        std::snprintf(backend, static_cast<size_t>(backend_len), "%s", reg_name ? reg_name : "");
        size_t free_mem = 0;
        size_t total_mem = 0;
        ggml_backend_dev_memory(dev, &free_mem, &total_mem);
        if (free_bytes) {
            *free_bytes = static_cast<std::uint64_t>(free_mem);
        }
        if (total_bytes) {
            *total_bytes = static_cast<std::uint64_t>(total_mem);
        }
        if (type) {
            *type = dev_type == GGML_BACKEND_DEVICE_TYPE_IGPU ? 1 : 0;
        }
        return 0;
    }
    return -1;
}

// 创建句柄。返回 nullptr 表示失败，错误信息写入 err。
void *ds_llama_create(const char *model_path, int n_ctx, int n_threads,
                      const char *device_name, char *err, int err_len) {
    if (!model_path || !*model_path) {
        set_err(err, err_len, "模型路径为空");
        return nullptr;
    }

    ensure_backend_initialized();
    // llama.cpp 默认把 graph/内存布局逐行打到 stderr（一次编码上千行），
    // 桌面端没有控制台，这些日志会拖慢编码并污染日志文件，直接静音。
    llama_log_set([](ggml_log_level, const char *, void *) {}, nullptr);

    llama_model_params mparams = llama_model_default_params();
    // CPU model-layer execution is intentionally disabled. `-1` means all
    // layers are assigned to the selected GGML accelerator.
    mparams.n_gpu_layers = -1;
    ggml_backend_dev_t selected_devices[2] = { nullptr, nullptr };
    if (device_name && *device_name) {
        ggml_backend_dev_t selected = nullptr;
        for (size_t i = 0; i < ggml_backend_dev_count(); ++i) {
            auto candidate = ggml_backend_dev_get(i);
            const auto type = ggml_backend_dev_type(candidate);
            if ((type == GGML_BACKEND_DEVICE_TYPE_GPU || type == GGML_BACKEND_DEVICE_TYPE_IGPU) &&
                device_key(candidate) == device_name) {
                selected = candidate;
                break;
            }
        }
        if (!selected) {
            set_err(err, err_len, "璇烽€夋嫨鐨勬樉鍗′笉鍦ㄥ綋鍓嶅悗绔敮鎸佺殑璁惧鍒楄〃涓�");
            return nullptr;
        }
        const auto type = ggml_backend_dev_type(selected);
        if (type != GGML_BACKEND_DEVICE_TYPE_GPU && type != GGML_BACKEND_DEVICE_TYPE_IGPU) {
            set_err(err, err_len, "鎸囧畾璁惧涓嶆槸 GPU 鎴栭泦鎴愭樉鍗�");
            return nullptr;
        }
        selected_devices[0] = selected;
        mparams.devices = selected_devices;
        mparams.split_mode = LLAMA_SPLIT_MODE_NONE;
        mparams.main_gpu = 0;
    }

    llama_model *model = llama_model_load_from_file(model_path, mparams);
    if (!model) {
        set_err(err, err_len, "llama_model_load_from_file 失败（文件不存在或不是合法 GGUF）");
        return nullptr;
    }

    llama_context_params cparams = llama_context_default_params();
    cparams.n_ctx = static_cast<uint32_t>(n_ctx > 0 ? n_ctx : 2048);
    // 新版 llama.cpp 对 encoder 模型有硬断言：
    //   GGML_ASSERT(cparams.n_ubatch >= n_tokens && "encoder requires n_ubatch >= n_tokens")
    // 一旦传入的文本 token 数超过 n_ubatch，进程会被直接 abort（不是返回错误码），
    // 表现为「索引到某个长文档时窗口突然消失」。这里把 batch 拉到上下文长度上限，
    // 并在 ds_llama_embed 里按 n_batch 截断，保证永远不会越界。
    const uint32_t batch_cap = std::max<uint32_t>(cparams.n_ctx, 512u);
    cparams.n_batch = batch_cap;
    cparams.n_ubatch = batch_cap;
    // A zero thread setting means "automatic". The previous fallback of four
    // threads left tokenisation, prompt preparation and CPU-side kernels
    // underutilised on ordinary desktop CPUs. Keep an explicit user value
    // authoritative, otherwise use all logical cores and give batched work a
    // little more room to overlap host-side preparation with GPU execution.
    const int automatic_threads = static_cast<int>(std::max(1u, std::thread::hardware_concurrency()));
    const int threads = n_threads > 0 ? n_threads : automatic_threads;
    cparams.n_threads = threads;
    cparams.n_threads_batch = n_threads > 0
        ? n_threads
        : std::max(threads, std::min(threads * 2, 64));
    cparams.embeddings = true;
    cparams.pooling_type = LLAMA_POOLING_TYPE_MEAN;
    cparams.offload_kqv = true;

    llama_context *ctx = llama_init_from_model(model, cparams);
    if (!ctx) {
        llama_model_free(model);
        set_err(err, err_len, "llama_init_from_model 失败（显存不足或上下文参数不支持）");
        return nullptr;
    }

    auto *h = new Handle();
    h->model = model;
    h->ctx = ctx;
    h->vocab = llama_model_get_vocab(model);
    // 注意：要用 **输出** 维度，不是隐层维度。
    // EmbeddingGemma 2 的 embedding_length = 512、embedding_length_out = 768；
    // llama_get_embeddings* 返回的是投影后的 768 维向量，若按 512 拷贝就会
    // 越界写坏堆，表现为「用一会儿莫名崩溃 0xc0000409 in ucrtbase.dll」。
    h->n_batch = static_cast<int>(cparams.n_batch);
    h->dim = llama_model_n_embd_out(model);
    if (h->dim <= 0) {
        h->dim = llama_model_n_embd(model);
    }
    if (h->dim <= 0) {
        llama_free(ctx);
        llama_model_free(model);
        delete h;
        set_err(err, err_len, "模型未报告 embedding 维度");
        return nullptr;
    }
    return h;
}

int ds_llama_dim(void *handle) {
    auto *h = static_cast<Handle *>(handle);
    return h ? h->dim : 0;
}

// 对一段文本求 embedding。is_query != 0 时按查询侧处理。
// out 由调用方分配，长度必须 >= dim；返回实际写入的维度，<0 表示失败。
int ds_llama_embed(void *handle, const char *text, int is_query, float *out,
                   int out_len, char *err, int err_len) {
    auto *h = static_cast<Handle *>(handle);
    if (!h || !h->ctx) {
        set_err(err, err_len, "句柄无效");
        return -1;
    }
    if (!out || out_len < h->dim) {
        set_err(err, err_len, "输出缓冲区太小");
        return -1;
    }
    const char *body = text ? text : "";
    if (std::strlen(body) == 0) {
        set_err(err, err_len, "文本为空");
        return -1;
    }

    // EmbeddingGemma 系列是「带任务前缀」训练的：文档与查询必须用不同前缀，
    // 否则向量空间塌陷、无关文本余弦也会很高（表现为「语义搜索什么都能搜到」
    // 或「怎么都搜不到」）。llama.cpp 不会自动加，这里补上。
    std::string src;
    if (is_query) {
        src = "task: search result | query: ";
    } else {
        src = "title: none | text: ";
    }
    src += body;

    const int src_len = static_cast<int>(src.size());

    // 先问需要多少 token
    int32_t n = llama_tokenize(h->vocab, src.c_str(), src_len, nullptr, 0, true, false);
    if (n == INT32_MIN) {
        set_err(err, err_len, "分词溢出");
        return -1;
    }
    if (n < 0) {
        n = -n;  // 返回负值表示需要的缓冲区大小
    }
    if (n == 0) {
        set_err(err, err_len, "分词结果为空");
        return -1;
    }

    std::vector<llama_token> tokens(static_cast<size_t>(n));
    const int32_t got = llama_tokenize(h->vocab, src.c_str(), src_len, tokens.data(), n, true, false);
    if (got <= 0) {
        set_err(err, err_len, "分词失败");
        return -1;
    }
    tokens.resize(static_cast<size_t>(got));

    // 单条 decode 的 token 数不能超过 n_batch/n_ubatch，否则新版 llama.cpp 会在
    // encoder 断言里直接 abort。超长文本按 batch 上限截断（embedding 的常规做法：
    // 保留开头，开头对语义贡献最大）。
    if (h->n_batch > 0 && got > h->n_batch) {
        tokens.resize(static_cast<size_t>(h->n_batch));
    }

    llama_batch batch = llama_batch_get_one(tokens.data(), static_cast<int32_t>(tokens.size()));
    const int32_t rc = llama_decode(h->ctx, batch);
    if (rc != 0) {
        char buf[160];
        std::snprintf(buf, sizeof(buf), "llama_decode 返回 %d（上下文长度不足或内存不足）", rc);
        set_err(err, err_len, buf);
        return -1;
    }

    float *emb = llama_get_embeddings_seq(h->ctx, 0);
    if (!emb) {
        emb = llama_get_embeddings(h->ctx);
    }
    if (!emb) {
        set_err(err, err_len, "模型没有返回 embedding（可能缺少 pooling 配置）");
        return -1;
    }

    // L2 归一化，方便后续直接用点积当余弦
    double sum = 0.0;
    for (int i = 0; i < h->dim; ++i) {
        sum += static_cast<double>(emb[i]) * static_cast<double>(emb[i]);
    }
    const double norm = std::sqrt(sum);
    const float inv = norm > 1e-12 ? static_cast<float>(1.0 / norm) : 0.0f;
    for (int i = 0; i < h->dim; ++i) {
        out[i] = emb[i] * inv;
    }

    // 清掉上一轮的 KV/state，避免下一次 decode 累积
    llama_memory_t mem = llama_get_memory(h->ctx);
    if (mem) {
        llama_memory_clear(mem, true);
    }

    return h->dim;
}

void ds_llama_free(void *handle) {
    auto *h = static_cast<Handle *>(handle);
    if (!h) {
        return;
    }
    if (h->ctx) {
        llama_free(h->ctx);
    }
    if (h->model) {
        llama_model_free(h->model);
    }
    delete h;
}

}  // extern "C"
