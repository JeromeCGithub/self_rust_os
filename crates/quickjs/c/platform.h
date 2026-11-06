/* Read host ABI declarations before remapping engine-only C symbols. */
#ifndef RUSTOS_QUICKJS_PLATFORM_H
#define RUSTOS_QUICKJS_PLATFORM_H
#include <assert.h>
#include <malloc.h>
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/time.h>
#include <time.h>
extern __typeof__(malloc) rjs_malloc;
extern __typeof__(calloc) rjs_calloc;
extern __typeof__(realloc) rjs_realloc;
extern __typeof__(free) rjs_free;
extern __typeof__(malloc_usable_size) rjs_malloc_usable_size;
extern __typeof__(abort) rjs_abort;
extern __typeof__(snprintf) rjs_snprintf;
extern __typeof__(vsnprintf) rjs_vsnprintf;
extern __typeof__(printf) rjs_printf;
extern __typeof__(fprintf) rjs_fprintf;
extern __typeof__(strlen) rjs_strlen;
extern __typeof__(strcmp) rjs_strcmp;
extern __typeof__(strchr) rjs_strchr;
extern __typeof__(strrchr) rjs_strrchr;
extern __typeof__(memchr) rjs_memchr;
extern __typeof__(abs) rjs_abs;
extern __typeof__(gettimeofday) rjs_gettimeofday;
extern __typeof__(localtime_r) rjs_localtime_r;
extern __typeof__(clock_gettime) rjs_clock_gettime;
extern __typeof__(__assert_fail) rjs___assert_fail;
extern __typeof__(acos) rjs_acos;
extern __typeof__(acosh) rjs_acosh;
extern __typeof__(asin) rjs_asin;
extern __typeof__(asinh) rjs_asinh;
extern __typeof__(atan) rjs_atan;
extern __typeof__(atan2) rjs_atan2;
extern __typeof__(atanh) rjs_atanh;
extern __typeof__(cbrt) rjs_cbrt;
extern __typeof__(ceil) rjs_ceil;
extern __typeof__(cos) rjs_cos;
extern __typeof__(cosh) rjs_cosh;
extern __typeof__(exp) rjs_exp;
extern __typeof__(expm1) rjs_expm1;
extern __typeof__(fabs) rjs_fabs;
extern __typeof__(floor) rjs_floor;
extern __typeof__(fmax) rjs_fmax;
extern __typeof__(fmin) rjs_fmin;
extern __typeof__(fmod) rjs_fmod;
extern __typeof__(hypot) rjs_hypot;
extern __typeof__(log) rjs_log;
extern __typeof__(log10) rjs_log10;
extern __typeof__(log1p) rjs_log1p;
extern __typeof__(log2) rjs_log2;
extern __typeof__(lrint) rjs_lrint;
extern __typeof__(modf) rjs_modf;
extern __typeof__(pow) rjs_pow;
extern __typeof__(round) rjs_round;
extern __typeof__(sin) rjs_sin;
extern __typeof__(sinh) rjs_sinh;
extern __typeof__(sqrt) rjs_sqrt;
extern __typeof__(tan) rjs_tan;
extern __typeof__(tanh) rjs_tanh;
extern __typeof__(trunc) rjs_trunc;
#define malloc rjs_malloc
#define calloc rjs_calloc
#define realloc rjs_realloc
#define free rjs_free
#define malloc_usable_size rjs_malloc_usable_size
#define abort rjs_abort
#define snprintf(...) rjs_snprintf(__VA_ARGS__)
#define vsnprintf(...) rjs_vsnprintf(__VA_ARGS__)
#define printf(...) rjs_printf(__VA_ARGS__)
#define fprintf(...) rjs_fprintf(__VA_ARGS__)
#define strlen rjs_strlen
#define strcmp rjs_strcmp
#define strchr rjs_strchr
#define strrchr rjs_strrchr
#define memchr rjs_memchr
#define abs rjs_abs
#define gettimeofday rjs_gettimeofday
#define localtime_r rjs_localtime_r
#define clock_gettime rjs_clock_gettime
#define __assert_fail rjs___assert_fail
#define acos rjs_acos
#define acosh rjs_acosh
#define asin rjs_asin
#define asinh rjs_asinh
#define atan rjs_atan
#define atan2 rjs_atan2
#define atanh rjs_atanh
#define cbrt rjs_cbrt
#define ceil rjs_ceil
#define cos rjs_cos
#define cosh rjs_cosh
#define exp rjs_exp
#define expm1 rjs_expm1
#define fabs rjs_fabs
#define floor rjs_floor
#define fmax rjs_fmax
#define fmin rjs_fmin
#define fmod rjs_fmod
#define hypot rjs_hypot
#define log rjs_log
#define log10 rjs_log10
#define log1p rjs_log1p
#define log2 rjs_log2
#define lrint rjs_lrint
#define modf rjs_modf
#define pow rjs_pow
#define round rjs_round
#define sin rjs_sin
#define sinh rjs_sinh
#define sqrt rjs_sqrt
#define tan rjs_tan
#define tanh rjs_tanh
#define trunc rjs_trunc
#endif
