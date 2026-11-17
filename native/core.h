// Copyright (C) 2026 Stanislav Yaglo
// SPDX-License-Identifier: Apache-2.0
#pragma once
#include <stddef.h>
#include <stdint.h>

// Opaque session owner. All calls run on its creating thread. Text and cell
// views are borrowed until the next call on the owner; Rust copies them before
// returning to its event loop. No C++ layout crosses this ABI, and exceptions are not caught: an
// allocation failure terminates the process.
#ifdef __cplusplus
extern "C" {
#endif
typedef struct UBQWorkspace UBQWorkspace;
typedef struct {
    const char* text;
    size_t length;
    uint32_t foreground, background, underline;
    uint8_t attributes, width, underline_style;
    const char* hyperlink;
    size_t hyperlink_length;
} UBQCell;
typedef struct {
    uint32_t image_id;
    float x, y, width, height, u0, v0, u1, v1;
    int32_t z;
    uint8_t filter;
} UBQQuad;
typedef struct {
    const uint8_t* pixels;
    size_t length;
    uint64_t stamp;
    uint32_t width, height, format;
} UBQImage;
typedef struct {
    const UBQCell* cells;
    size_t count;
    int columns, rows, cursor_column, cursor_row;
    uint8_t cursor_visible, application_cursor, application_keypad;
    uint8_t bracketed_paste, cursor_style;
    uint16_t keyboard_flags;
    const UBQQuad* quads;
    size_t quad_count;
} UBQFrame;
typedef struct {
    const char* verb;
    uint32_t surface_id, tab_id, pane_id, target_tab_id, target_pane_id;
    uint8_t direction, orientation, side;
    double ratio;
    const uint8_t* title;
    size_t title_length;
} UBQIntent;
UBQWorkspace* ubq_open(const char* host, const char* ssh, const char* socket,
    uint32_t session, uint32_t workspace, uint64_t daemon_instance, const char* name,
    int columns, int rows, int graphics);
void ubq_close(UBQWorkspace*);
int ubq_tick(UBQWorkspace*);
int ubq_dirty(const UBQWorkspace*);
int ubq_wait_descriptor(const UBQWorkspace*);
int ubq_wait_timeout(const UBQWorkspace*);
const char* ubq_snapshot(UBQWorkspace*);
void ubq_resize(UBQWorkspace*, int columns, int rows);
void ubq_input(UBQWorkspace*, const uint8_t*, size_t);
int ubq_intent(UBQWorkspace*, const UBQIntent*);
void ubq_watch_directory(UBQWorkspace*);
void ubq_set_view(UBQWorkspace*, uint32_t session, int columns, int rows, int focused);
void ubq_pixels(UBQWorkspace*, uint32_t session, int columns, int rows, int cell_width, int cell_height);
void ubq_mouse(UBQWorkspace*, uint32_t session, int kind, int button, int column, int row, int modifiers, int pixel_x, int pixel_y);
UBQImage ubq_image(UBQWorkspace*, uint32_t session, uint32_t image);
void ubq_attach(UBQWorkspace*, uint32_t session, uint32_t workspace);
void ubq_reveal(UBQWorkspace*, uint32_t session);
UBQFrame ubq_frame(UBQWorkspace*, uint32_t session);
char* ubq_dump(const uint8_t*, size_t, int columns, int rows, int metadata);
// Lists the host's workspaces once, without attaching, as JSON the caller
// frees with ubq_text_free. Failure is reported in the JSON's "error".
char* ubq_directory(const char* host, const char* ssh, const char* socket, int timeout_milliseconds);
// Asks the local daemon serving `socket` to stop if it holds nothing, after
// checking it runs as this user from `host`. Returns whether it was asked.
int ubq_retire_idle_daemon(const char* host, const char* socket);
void ubq_text_free(char*);
#ifdef __cplusplus
}
#endif
