// luma-llm: writes Tech weather headlines with a small language model.
//
// Luma starts it only when it needs new headlines; it loads the model, writes
// one line per request and exits, so the model's memory is returned at once.
//
//   luma-llm MODEL THREADS SEED < input
//
// input: the prompt (examples), then a line "%%", then one request per line
// ("rain (firewall):"). Each request is written after the prompt on its own
// (the prompt is evaluated once and kept), and its answer is printed as
// "= <text>". Timings and peak memory go to stderr as "stat ..." lines.
#include "llama.h"

#include <sys/resource.h>
#include <sys/time.h>

#include <chrono>
#include <clocale>
#include <cstdio>
#include <cstdlib>
#include <iostream>
#include <string>
#include <vector>

namespace {

double now_ms() {
    using namespace std::chrono;
    return duration<double, std::milli>(steady_clock::now().time_since_epoch()).count();
}

long peak_rss_kib() {
    rusage usage{};
    getrusage(RUSAGE_SELF, &usage);
    return usage.ru_maxrss;
}

std::vector<llama_token> tokenize(const llama_vocab * vocab, const std::string & text, bool first) {
    int n = -llama_tokenize(vocab, text.c_str(), text.size(), nullptr, 0, first, false);
    std::vector<llama_token> tokens(n > 0 ? n : 0);
    if (n > 0 && llama_tokenize(vocab, text.c_str(), text.size(), tokens.data(), n, first, false) < 0) {
        tokens.clear();
    }
    return tokens;
}

// Feeds tokens in chunks of at most `batch`, starting at position `pos`.
bool feed(llama_context * ctx, std::vector<llama_token> & tokens, int batch) {
    for (size_t at = 0; at < tokens.size(); at += batch) {
        int n = static_cast<int>(std::min<size_t>(batch, tokens.size() - at));
        if (llama_decode(ctx, llama_batch_get_one(tokens.data() + at, n)) != 0) {
            return false;
        }
    }
    return true;
}

}  // namespace

int main(int argc, char ** argv) {
    if (argc < 4) {
        std::fprintf(stderr, "usage: %s MODEL THREADS SEED < input\n", argv[0]);
        return 2;
    }
    std::setlocale(LC_NUMERIC, "C");
    // Background work: never compete with the launcher's own frames.
    setpriority(PRIO_PROCESS, 0, 19);
    const int threads = std::max(1, std::atoi(argv[2]));
    const uint32_t seed = static_cast<uint32_t>(std::strtoul(argv[3], nullptr, 10));

    std::string prompt;
    std::vector<std::string> requests;
    {
        std::string line;
        bool in_requests = false;
        while (std::getline(std::cin, line)) {
            if (!in_requests && line == "%%") {
                in_requests = true;
            } else if (in_requests) {
                if (!line.empty()) requests.push_back(line);
            } else {
                prompt += line;
                prompt += '\n';
            }
        }
    }
    if (prompt.empty() || requests.empty()) {
        std::fprintf(stderr, "luma-llm: nothing to write\n");
        return 2;
    }

    llama_log_set([](ggml_log_level level, const char * text, void *) {
        if (level >= GGML_LOG_LEVEL_ERROR) std::fputs(text, stderr);
    }, nullptr);
    llama_backend_init();

    const double started = now_ms();
    llama_model_params model_params = llama_model_default_params();
    model_params.n_gpu_layers = 0;
    model_params.progress_callback = [](float, void *) { return true; };
    llama_model * model = llama_model_load_from_file(argv[1], model_params);
    if (!model) {
        std::fprintf(stderr, "luma-llm: cannot load %s\n", argv[1]);
        return 1;
    }
    const llama_vocab * vocab = llama_model_get_vocab(model);
    std::vector<llama_token> prefix = tokenize(vocab, prompt, true);

    llama_context_params ctx_params = llama_context_default_params();
    ctx_params.n_ctx = static_cast<uint32_t>(prefix.size()) + 160;
    ctx_params.n_batch = 256;
    ctx_params.n_ubatch = 256;
    ctx_params.n_threads = threads;
    ctx_params.n_threads_batch = threads;
    ctx_params.no_perf = true;
    llama_context * ctx = llama_init_from_model(model, ctx_params);
    if (!ctx) {
        std::fprintf(stderr, "luma-llm: cannot create a context\n");
        return 1;
    }
    const double loaded = now_ms();
    if (prefix.empty() || !feed(ctx, prefix, 256)) {
        std::fprintf(stderr, "luma-llm: cannot read the prompt\n");
        return 1;
    }
    const double primed = now_ms();
    std::fprintf(stderr, "stat load_ms=%.0f prompt_tokens=%zu prompt_ms=%.0f\n",
                 loaded - started, prefix.size(), primed - loaded);

    llama_sampler * sampler = llama_sampler_chain_init(llama_sampler_chain_default_params());
    llama_sampler_chain_add(sampler, llama_sampler_init_penalties(llama_vocab_n_tokens(vocab), 64, 1.12f, 0.0f, 0.0f));
    llama_sampler_chain_add(sampler, llama_sampler_init_top_p(0.95f, 1));
    llama_sampler_chain_add(sampler, llama_sampler_init_temp(0.9f));
    llama_sampler_chain_add(sampler, llama_sampler_init_dist(seed));

    llama_memory_t memory = llama_get_memory(ctx);
    const llama_pos kept = static_cast<llama_pos>(prefix.size());
    int written = 0;
    for (const std::string & request : requests) {
        const double begun = now_ms();
        // Forget the previous answer; keep the evaluated prompt.
        llama_memory_seq_rm(memory, 0, kept, -1);
        llama_sampler_reset(sampler);
        std::vector<llama_token> ask = tokenize(vocab, request, false);
        if (ask.empty() || !feed(ctx, ask, 256)) break;
        std::string answer;
        int produced = 0;
        for (; produced < 48; ++produced) {
            llama_token token = llama_sampler_sample(sampler, ctx, -1);
            if (llama_vocab_is_eog(vocab, token)) break;
            char piece[128];
            int n = llama_token_to_piece(vocab, token, piece, sizeof(piece), 0, false);
            if (n < 0) break;
            std::string text(piece, n);
            size_t newline = text.find('\n');
            if (newline != std::string::npos) {
                answer += text.substr(0, newline);
                break;
            }
            answer += text;
            if (llama_decode(ctx, llama_batch_get_one(&token, 1)) != 0) break;
        }
        std::printf("= %s\n", answer.c_str());
        std::fflush(stdout);
        ++written;
        std::fprintf(stderr, "stat line_tokens=%d line_ms=%.0f\n", produced, now_ms() - begun);
    }
    std::fprintf(stderr, "stat lines=%d total_ms=%.0f peak_rss_kib=%ld\n", written,
                 now_ms() - started, peak_rss_kib());

    llama_sampler_free(sampler);
    llama_free(ctx);
    llama_model_free(model);
    llama_backend_free();
    return 0;
}
