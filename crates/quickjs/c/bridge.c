/* Narrow C ABI: JSValue/layout and vendor lifetimes do not cross into Rust. */
#include "quickjs.h"
#include <malloc.h>
#include <stdlib.h>
#include <string.h>

typedef int (*HostCall)(void *, uint32_t, uint32_t, const char *, size_t,
                        const char *, size_t, char *, size_t, size_t *, uint32_t *);
typedef struct {
  void *host;
  HostCall host_call;
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
int rjs_date(void *opaque) {
  Probe *p = opaque;
  return JS_AddIntrinsicDate(p->context) ? 3 : 0;
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

/* Script completion values are deliberately never converted to strings. */
int rjs_execute(void *opaque, const char *source, size_t length) {
  Probe *p = opaque;
  p->polls = p->quota_failed = p->interrupted = 0;
  JS_UpdateStackTop(p->runtime);
  JSValue value = JS_Eval(p->context, source, length, "page.js", JS_EVAL_TYPE_GLOBAL);
  int status = 0;
  if (JS_IsException(value)) {
    JSValue error = JS_GetException(p->context);
    JS_FreeValue(p->context, error);
    status = p->interrupted ? 2 : p->quota_failed ? 3 : 1;
  }
  JS_FreeValue(p->context, value);
  JS_RunGC(p->runtime);
  return status;
}

/* One CPU budget covers the entire checkpoint, including all jobs. */
int rjs_jobs(void *opaque, size_t limit, size_t *completed) {
  Probe *p = opaque;
  p->polls = p->quota_failed = p->interrupted = 0;
  *completed = 0;
  JS_UpdateStackTop(p->runtime);
  while (*completed < limit && JS_IsJobPending(p->runtime)) {
    JSContext *context = NULL;
    int status = JS_ExecutePendingJob(p->runtime, &context);
    (*completed)++;
    if (status < 0) {
      if (context) {
        JSValue error = JS_GetException(context);
        JS_FreeValue(context, error);
      }
      return p->interrupted ? 2 : p->quota_failed ? 3 : 1;
    }
    /* A Promise rejection may consume the exception while the interrupt/quota
       flag still identifies an exhausted checkpoint. */
    if (p->interrupted || p->quota_failed)
      return p->interrupted ? 2 : 3;
  }
  JS_RunGC(p->runtime);
  return JS_IsJobPending(p->runtime) ? 2 : 0;
}

/* All conversion happens before entering Rust, and all host results are copied
   before returning to JS. No JS value crosses this boundary. */
static JSValue host_call(JSContext *context, JSValueConst this_value, int argc,
                         JSValueConst *argv) {
  (void)this_value;
  Probe *p = JS_GetContextOpaque(context);
  uint32_t operation, node, result_node = 0;
  if (!p || !p->host_call || argc != 4 ||
      JS_ToUint32(context, &operation, argv[0]) ||
      JS_ToUint32(context, &node, argv[1]))
    return JS_ThrowTypeError(context, "invalid host call");
  size_t a_len = 0, b_len = 0, written = 0;
  const char *a = JS_ToCStringLen(context, &a_len, argv[2]);
  if (!a) return JS_EXCEPTION;
  const char *b = JS_ToCStringLen(context, &b_len, argv[3]);
  if (!b) { JS_FreeCString(context, a); return JS_EXCEPTION; }
  /* Rust receives &mut [u8], which requires initialized bytes even though
     the callback only overwrites the returned prefix. */
  char output[8192] = {0};
  int status = -1;
  if (a_len <= sizeof(output) && b_len <= sizeof(output))
    status = p->host_call(p->host, operation, node, a, a_len, b, b_len,
                          output, sizeof(output), &written, &result_node);
  JS_FreeCString(context, a);
  JS_FreeCString(context, b);
  if (status == 0 && written <= sizeof(output))
    return JS_NewStringLen(context, output, written);
  if (status == 1) return JS_NewUint32(context, result_node);
  if (status == 2) return JS_NULL;
  if (status == 3) return JS_UNDEFINED;
  return JS_ThrowTypeError(context, "host operation rejected");
}
int rjs_host(void *opaque, void *host, HostCall callback) {
  Probe *p = opaque;
  if (p->host || !host || !callback) return -1;
  p->host = host;
  p->host_call = callback;
  JS_SetContextOpaque(p->context, p);
  JSValue global = JS_GetGlobalObject(p->context);
  int result = JS_SetPropertyStr(p->context, global, "__rustosHost",
                                 JS_NewCFunction(p->context, host_call, "", 4));
  JS_FreeValue(p->context, global);
  return result < 0 ? -1 : 0;
}
