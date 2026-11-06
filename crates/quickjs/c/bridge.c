/* Narrow C ABI: JSValue/layout and vendor lifetimes do not cross into Rust. */
#include "quickjs.h"
#include <malloc.h>
#include <stdlib.h>
#include <string.h>

typedef struct {
  JSRuntime *runtime;
  JSContext *context;
  size_t current, peak, allocations;
  uint64_t polls, budget;
  int quota_failed, interrupted;
} Probe;
typedef struct {
  size_t allocated_bytes, peak_bytes, allocations;
  uint64_t interrupt_polls;
} ProbeStats;

static void *limited_malloc(JSMallocState *state, size_t size) {
  Probe *p = state->opaque;
  if (!size || state->malloc_size > state->malloc_limit ||
      size > state->malloc_limit - state->malloc_size) {
    p->quota_failed = 1;
    return NULL;
  }
  void *pointer = malloc(size);
  if (!pointer) {
    p->quota_failed = 1;
    return NULL;
  }
  size_t actual = malloc_usable_size(pointer);
  if (actual > state->malloc_limit - state->malloc_size) {
    free(pointer);
    p->quota_failed = 1;
    return NULL;
  }
  state->malloc_count++;
  state->malloc_size += actual;
  p->current = state->malloc_size;
  p->allocations = state->malloc_count;
  if (p->current > p->peak)
    p->peak = p->current;
  return pointer;
}
static void limited_free(JSMallocState *state, void *pointer) {
  if (!pointer)
    return;
  Probe *p = state->opaque;
  state->malloc_count--;
  state->malloc_size -= malloc_usable_size(pointer);
  free(pointer);
  p->current = state->malloc_size;
  p->allocations = state->malloc_count;
}
static void *limited_realloc(JSMallocState *state, void *old, size_t size) {
  if (!old)
    return limited_malloc(state, size);
  if (!size) {
    limited_free(state, old);
    return NULL;
  }
  /* Conservative peak budget: preserve the old block until the new one exists.
   */
  void *next = limited_malloc(state, size);
  if (!next)
    return NULL;
  size_t length = malloc_usable_size(old);
  memcpy(next, old, length < size ? length : size);
  limited_free(state, old);
  return next;
}
static size_t usable(const void *pointer) {
  return pointer ? malloc_usable_size((void *)pointer) : 0;
}
static int interrupt(JSRuntime *rt, void *opaque) {
  (void)rt;
  Probe *p = opaque;
  if (++p->polls >= p->budget) {
    p->interrupted = 1;
    return 1;
  }
  return 0;
}
void rjs_destroy(void *opaque) {
  Probe *p = opaque;
  if (p->context)
    JS_FreeContext(p->context);
  if (p->runtime)
    JS_FreeRuntime(p->runtime);
  free(p);
}
void *rjs_create(size_t memory, size_t stack, uint64_t budget) {
  Probe *p = calloc(1, sizeof(*p));
  if (!p)
    return NULL;
  const JSMallocFunctions functions = {limited_malloc, limited_free,
                                       limited_realloc, usable};
  p->budget = budget;
  p->runtime = JS_NewRuntime2(&functions, p);
  if (!p->runtime) {
    free(p);
    return NULL;
  }
  JS_SetMemoryLimit(p->runtime, memory);
  JS_SetMaxStackSize(p->runtime, stack);
  JS_SetInterruptHandler(p->runtime, interrupt, p);
  p->context = JS_NewContextRaw(p->runtime);
  if (!p->context) {
    rjs_destroy(p);
    return NULL;
  }
  /* No Date, blocking Atomics, OS modules, module loader or browser bindings.
   */
  if (JS_AddIntrinsicBaseObjects(p->context) ||
      JS_AddIntrinsicEval(p->context) ||
      JS_AddIntrinsicStringNormalize(p->context) ||
      JS_AddIntrinsicRegExp(p->context) || JS_AddIntrinsicJSON(p->context) ||
      JS_AddIntrinsicProxy(p->context) || JS_AddIntrinsicMapSet(p->context) ||
      JS_AddIntrinsicTypedArrays(p->context) ||
      JS_AddIntrinsicPromise(p->context) ||
      JS_AddIntrinsicWeakRef(p->context)) {
    rjs_destroy(p);
    return NULL;
  }
  return p;
}
int rjs_eval(void *opaque, const char *source, size_t length, char *output,
             size_t capacity, size_t *written) {
  Probe *p = opaque;
  p->polls = 0;
  p->quota_failed = 0;
  p->interrupted = 0;
  *written = 0;
  JS_UpdateStackTop(p->runtime);
  JSValue value =
      JS_Eval(p->context, source, length, "probe.js", JS_EVAL_TYPE_GLOBAL);
  if (JS_IsException(value)) {
    JSValue error = JS_GetException(p->context);
    JS_FreeValue(p->context, error);
    JS_RunGC(p->runtime);
    return p->interrupted ? 2 : p->quota_failed ? 3 : 1;
  }
  size_t size = 0;
  const char *text = JS_ToCStringLen(p->context, &size, value);
  if (!text) {
    JS_FreeValue(p->context, value);
    JSValue error = JS_GetException(p->context);
    JS_FreeValue(p->context, error);
    return p->interrupted ? 2 : p->quota_failed ? 3 : 1;
  }
  int status = 0;
  if (size > capacity)
    status = 4;
  else {
    memcpy(output, text, size);
    *written = size;
  }
  JS_FreeCString(p->context, text);
  JS_FreeValue(p->context, value);
  JS_RunGC(p->runtime);
  return status;
}
void rjs_stats(const void *opaque, ProbeStats *stats) {
  const Probe *p = opaque;
  *stats = (ProbeStats){p->current, p->peak, p->allocations, p->polls};
}
