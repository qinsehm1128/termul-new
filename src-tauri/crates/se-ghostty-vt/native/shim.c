/* Tiny ABI firewall: upstream structs/enums stay in C, not hand-copied Rust. */
#include <ghostty/vt.h>
#include <stdint.h>
#include <string.h>

struct se_vt_meta {
    uint16_t cols, rows, cursor_x, cursor_y;
    uint8_t alternate_screen;
};

void *se_vt_new(uint16_t cols, uint16_t rows) {
    /* This source commit reports the standalone VT ABI as 0.1.0-dev;
       -Dversion-string configures the Ghostty app, not this C API. Source
       identity is verified by build.rs (and explicit prebuild provenance). */
    static const char pinned_version[] = "0.1.0-dev";
    GhosttyString version = {0};
    if (ghostty_build_info(GHOSTTY_BUILD_INFO_VERSION_STRING, &version) != GHOSTTY_SUCCESS ||
        version.len != sizeof(pinned_version) - 1 ||
        memcmp(version.ptr, pinned_version, version.len) != 0) return NULL;

    GhosttyTerminal terminal = NULL;
    if (ghostty_terminal_new(NULL, &terminal, cols, rows) != GHOSTTY_SUCCESS) return NULL;

    const bool disabled = false;
    const uint64_t zero = 0;
    const size_t scrollback_bytes = 0;
    const size_t scrollback_lines = 0;
    const size_t apc_bytes = 4096;
    const size_t continuation_bytes = 4096;
    const size_t unknown_bytes = 0;
    /* No host-effect callbacks of ANY kind are ever installed. In particular:
       no WRITE_PTY, clipboard, bell, title, file loading or query callbacks.
       These are parser data limits, NOT host-effect callback registrations. */
#define SET(key, value) do { \
    if (ghostty_terminal_set(terminal, (key), (value)) != GHOSTTY_SUCCESS) goto fail; \
} while (0)
    SET(GHOSTTY_TERMINAL_OPT_KITTY_IMAGE_STORAGE_LIMIT, &zero);
    SET(GHOSTTY_TERMINAL_OPT_KITTY_IMAGE_MEDIUM_FILE, &disabled);
    SET(GHOSTTY_TERMINAL_OPT_KITTY_IMAGE_MEDIUM_TEMP_FILE, NULL);
    SET(GHOSTTY_TERMINAL_OPT_KITTY_IMAGE_MEDIUM_SHARED_MEM, &disabled);
    SET(GHOSTTY_TERMINAL_OPT_APC_MAX_BYTES, &apc_bytes);
    SET(GHOSTTY_TERMINAL_OPT_APC_MAX_BYTES_KITTY, &apc_bytes);
    SET(GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_BYTES, &scrollback_bytes);
    SET(GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_LINES, &scrollback_lines);
    SET(GHOSTTY_TERMINAL_OPT_CONTINUATION_MAX_BYTES, &continuation_bytes);
    SET(GHOSTTY_TERMINAL_OPT_UNKNOWN_MAX_BYTES, &unknown_bytes);
    SET(GHOSTTY_TERMINAL_OPT_GLYPH_PROTOCOL, &disabled);
#undef SET
    return terminal;
fail:
    ghostty_terminal_free(terminal);
    return NULL;
}

void se_vt_free(void *terminal) {
    ghostty_terminal_free(terminal);
}

int se_vt_write(void *terminal, const uint8_t *bytes, size_t len) {
    ghostty_terminal_vt_write(terminal, bytes, len);
    bool failed = false;
    if (ghostty_terminal_get(terminal, GHOSTTY_TERMINAL_DATA_VT_PROCESSING_ERROR, &failed) != GHOSTTY_SUCCESS || failed) return -1;
    return 0;
}

int se_vt_resize(void *terminal, uint16_t cols, uint16_t rows, uint32_t cell_width, uint32_t cell_height) {
    return ghostty_terminal_resize(terminal, cols, rows, cell_width, cell_height) == GHOSTTY_SUCCESS ? 0 : -1;
}

int se_vt_snapshot(void *terminal, uint8_t *bytes, size_t cap, size_t *len, struct se_vt_meta *meta) {
    GhosttyTerminalScreen screen;
#define GET(key, value) do { \
    if (ghostty_terminal_get(terminal, (key), (value)) != GHOSTTY_SUCCESS) return -1; \
} while (0)
    GET(GHOSTTY_TERMINAL_DATA_COLS, &meta->cols);
    GET(GHOSTTY_TERMINAL_DATA_ROWS, &meta->rows);
    GET(GHOSTTY_TERMINAL_DATA_CURSOR_X, &meta->cursor_x);
    GET(GHOSTTY_TERMINAL_DATA_CURSOR_Y, &meta->cursor_y);
    GET(GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN, &screen);
#undef GET
    meta->alternate_screen = screen == GHOSTTY_TERMINAL_SCREEN_ALTERNATE;
    GhosttyFormatter formatter = NULL;
    GhosttyFormatterTerminalOptions options = {
        .size = sizeof(GhosttyFormatterTerminalOptions),
        .emit = GHOSTTY_FORMATTER_FORMAT_PLAIN,
        .unwrap = false,
        .trim = true,
    };
    if (ghostty_formatter_terminal_new(NULL, &formatter, terminal, options) != GHOSTTY_SUCCESS) return -1;
    GhosttyResult result = ghostty_formatter_format_buf(formatter, bytes, cap, len);
    ghostty_formatter_free(formatter);
    return result == GHOSTTY_SUCCESS ? 0 : -1;
}
