#include "v2d.h"
#include <psp2/display.h>
#include <psp2/ime_dialog.h>
#include <psp2/kernel/sysmem.h>
#include <psp2/libime.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/time.h>
#include <vita2d.h>

typedef unsigned short u16;
typedef unsigned char u8;

char *get_format_time() {
  char *str = malloc(24);
  struct timeval tv;
  gettimeofday(&tv, NULL);
  time_t t = time(NULL);
  struct tm *lt = localtime(&t);
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wformat-truncation="
  snprintf(str, 24, "%04d-%02d-%02d %02d.%02d.%02d.%03ld", lt->tm_year + 1900,
           lt->tm_mon + 1, lt->tm_mday, lt->tm_hour, lt->tm_min, lt->tm_sec,
           tv.tv_usec / 1000);
#pragma GCC diagnostic pop
  return str;
}

static void utf8_to_utf16(const uint8_t *src, uint16_t *dst) {
  int i;
  for (i = 0; src[i];) {
    if ((src[i] & 0xE0) == 0xE0) {
      *(dst++) = ((src[i] & 0x0F) << 12) | ((src[i + 1] & 0x3F) << 6) |
                 (src[i + 2] & 0x3F);
      i += 3;
    } else if ((src[i] & 0xC0) == 0xC0) {
      *(dst++) = ((src[i] & 0x1F) << 6) | (src[i + 1] & 0x3F);
      i += 2;
    } else {
      *(dst++) = src[i];
      i += 1;
    }
  }

  *dst = '\0';
}

static void utf16_to_utf8(const uint16_t *src, uint8_t *dst) {
  int i;
  for (i = 0; src[i]; i++) {
    if ((src[i] & 0xFF80) == 0) {
      *(dst++) = src[i] & 0xFF;
    } else if ((src[i] & 0xF800) == 0) {
      *(dst++) = ((src[i] >> 6) & 0xFF) | 0xC0;
      *(dst++) = (src[i] & 0x3F) | 0x80;
    } else if ((src[i] & 0xFC00) == 0xD800 && (src[i + 1] & 0xFC00) == 0xDC00) {
      *(dst++) = (((src[i] + 64) >> 8) & 0x3) | 0xF0;
      *(dst++) = (((src[i] >> 2) + 16) & 0x3F) | 0x80;
      *(dst++) = ((src[i] >> 4) & 0x30) | 0x80 | ((src[i + 1] << 2) & 0xF);
      *(dst++) = (src[i + 1] & 0x3F) | 0x80;
      i += 1;
    } else {
      *(dst++) = ((src[i] >> 12) & 0xF) | 0xE0;
      *(dst++) = ((src[i] >> 6) & 0x3F) | 0x80;
      *(dst++) = (src[i] & 0x3F) | 0x80;
    }
  }

  *dst = '\0';
}

char *show_psv_ime(char *input_init) {
  char *res = NULL;
  u16 input[SCE_IME_DIALOG_MAX_TEXT_LENGTH + 1] = {0};
  u16 input_init_buf[SCE_IME_DIALOG_MAX_TEXT_LENGTH + 1] = {0};
  utf8_to_utf16((u8 *)input_init, input_init_buf);

  // common dialog param
  sceCommonDialogSetConfigParam(&(SceCommonDialogConfigParam){});

  // ime dialog param
  SceImeDialogParam param;
  sceImeDialogParamInit(&param);
  // config
  param.supportedLanguages =
      SCE_IME_LANGUAGE_ENGLISH;
  param.languagesForced = SCE_TRUE;
  param.type = SCE_IME_DIALOG_TEXTBOX_MODE_DEFAULT;
  param.option = 0;
  param.textBoxMode = SCE_IME_DIALOG_TEXTBOX_MODE_DEFAULT;
  param.title = u"Enter text";
  param.maxTextLength = SCE_IME_DIALOG_MAX_TEXT_LENGTH;
  param.initialText = input_init_buf;
  param.inputTextBuffer = input;
  // init ime dialog
  sceImeDialogInit(&param);

  // draw ime dialog
  vita2d_texture *screenshot_tex = v2d_get_full_screenshot();
  while (1) {
    // hide ime dialog
    if (sceImeDialogGetStatus() == SCE_COMMON_DIALOG_STATUS_FINISHED) {
      SceImeDialogResult result = {};
      sceImeDialogGetResult(&result);
      if (result.button == SCE_IME_DIALOG_BUTTON_ENTER) {
        res = malloc(SCE_IME_DIALOG_MAX_TEXT_LENGTH + 1);
        memset(res, 0, SCE_IME_DIALOG_MAX_TEXT_LENGTH + 1);
        utf16_to_utf8(input, (u8 *)res);
      }
      sceImeDialogTerm();
      break;
    }

    // vita2d start drawing
    vita2d_start_drawing();
    // clear screen
    vita2d_clear_screen();
    // screenshot bg
    v2d_draw_texture(screenshot_tex, 0.0, 0.0);
    // vita2d end drawing
    vita2d_end_drawing();
    // update common dialog display buffer
    vita2d_common_dialog_update();
    // swap buffers
    vita2d_swap_buffers();
    sceDisplayWaitVblankStart();
  }
  vita2d_free_texture(screenshot_tex);

  return res;
}

void ime_input_free(char *ptr) {
  if (ptr != NULL) {
    free(ptr);
  }
}
