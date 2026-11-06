/* Restricted x86-64 native ABI. Not a POSIX libc or a general filesystem layer.
 */
#include <malloc.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/time.h>
#define NANOPRINTF_IMPLEMENTATION
#define NANOPRINTF_USE_FIELD_WIDTH_FORMAT_SPECIFIERS 1
#define NANOPRINTF_USE_PRECISION_FORMAT_SPECIFIERS 1
#define NANOPRINTF_USE_FLOAT_FORMAT_SPECIFIERS 1
#define NANOPRINTF_USE_LARGE_FORMAT_SPECIFIERS 1
#define NANOPRINTF_USE_SMALL_FORMAT_SPECIFIERS 1
#define NANOPRINTF_USE_BINARY_FORMAT_SPECIFIERS 0
#define NANOPRINTF_USE_WRITEBACK_FORMAT_SPECIFIERS 0
#include "nanoprintf.h"

extern void *rjs_allocate(size_t size);
extern void rjs_release(void *pointer);
extern size_t rjs_allocation_size(const void *pointer);
extern uint64_t rustos_qjs_epoch_micros(void);
extern void rustos_qjs_abort(void) __attribute__((noreturn));
void *rjs_malloc(size_t size) { return rjs_allocate(size); }
void rjs_free(void *pointer) {
  if (pointer)
    rjs_release(pointer);
}
size_t rjs_malloc_usable_size(void *pointer) {
  return pointer ? rjs_allocation_size(pointer) : 0;
}
void *rjs_calloc(size_t count, size_t size) {
  size_t n;
  if (__builtin_mul_overflow(count, size, &n))
    return NULL;
  void *p = rjs_malloc(n);
  if (p)
    memset(p, 0, n);
  return p;
}
void *rjs_realloc(void *old, size_t size) {
  if (!old)
    return rjs_malloc(size);
  if (!size) {
    rjs_free(old);
    return NULL;
  }
  void *p = rjs_malloc(size);
  if (!p)
    return NULL;
  size_t n = rjs_malloc_usable_size(old);
  memcpy(p, old, n < size ? n : size);
  rjs_free(old);
  return p;
}
void rjs_abort(void) { rustos_qjs_abort(); }
void rjs___assert_fail(const char *assertion, const char *file, unsigned line,
                       const char *function) {
  (void)assertion;
  (void)file;
  (void)line;
  (void)function;
  rustos_qjs_abort();
}
int rjs_abs(int value) { return value < 0 ? -value : value; }
size_t rjs_strlen(const char *s) {
  size_t n = 0;
  while (s[n])
    n++;
  return n;
}
int rjs_strcmp(const char *a, const char *b) {
  while (*a && *a == *b) {
    a++;
    b++;
  }
  return (unsigned char)*a - (unsigned char)*b;
}
char *rjs_strchr(const char *s, int ch) {
  do {
    if ((unsigned char)*s == (unsigned char)ch)
      return (char *)s;
  } while (*s++);
  return NULL;
}
char *rjs_strrchr(const char *s, int ch) {
  const char *p = NULL;
  do {
    if ((unsigned char)*s == (unsigned char)ch)
      p = s;
  } while (*s++);
  return (char *)p;
}
void *rjs_memchr(const void *ptr, int ch, size_t count) {
  const unsigned char *p = ptr;
  for (size_t i = 0; i < count; i++)
    if (p[i] == (unsigned char)ch)
      return (void *)(p + i);
  return NULL;
}
int rjs_vsnprintf(char *s, size_t capacity, const char *format, va_list args) {
  return npf_vsnprintf(s, capacity, format, args);
}
int rjs_snprintf(char *s, size_t capacity, const char *format, ...) {
  va_list args;
  va_start(args, format);
  int n = rjs_vsnprintf(s, capacity, format, args);
  va_end(args);
  return n;
}
/* Debug dump output is unsupported and never sent page data to kernel logs. */
int rjs_printf(const char *format, ...) {
  (void)format;
  return -1;
}
int rjs_fprintf(FILE *file, const char *format, ...) {
  (void)file;
  (void)format;
  return -1;
}
/* Used only to seed Math.random; Date is not installed in this probe profile.
 */
int rjs_gettimeofday(struct timeval *value, void *zone) {
  (void)zone;
  uint64_t us = rustos_qjs_epoch_micros();
  value->tv_sec = us / 1000000;
  value->tv_usec = us % 1000000;
  return 0;
}

int rjs_platform_check(void) {
  char text[64];
  int n = rjs_snprintf(text, sizeof(text), "%08x %lld %.2f %.*s", 42, -17LL,
                       1.25, 3, "abcdef");
  if (n != 21 || rjs_strcmp(text, "0000002a -17 1.25 abc"))
    return 0;
  char short_text[3];
  if (rjs_snprintf(short_text, 3, "abcd") != 4 || rjs_strcmp(short_text, "ab"))
    return 0;
  unsigned char *p = rjs_calloc(16, 1);
  if (!p || ((uintptr_t)p & 15))
    return 0;
  for (size_t i = 0; i < 16; i++)
    if (p[i]) {
      rjs_free(p);
      return 0;
    }
  p[0] = 17;
  p[15] = 29;
  if (rjs_realloc(p, SIZE_MAX) != NULL) {
    rjs_free(p);
    return 0;
  }
  if (p[0] != 17 || p[15] != 29) {
    rjs_free(p);
    return 0;
  }
  unsigned char *q = rjs_realloc(p, 32);
  if (!q) {
    rjs_free(p);
    return 0;
  }
  int ok = q[0] == 17 && q[15] == 29;
  rjs_free(q);
  return ok;
}
