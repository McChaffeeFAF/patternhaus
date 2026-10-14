/* Patternhaus crash guard (M2/T3; DESIGN §6 "crash isolation").
 *
 * ph_guard_call(f, ctx, &code) runs f(ctx). When native code inside it faults (access
 * violation, illegal instruction, divide by zero, bus error), the call returns 1 with the
 * fault code instead of killing the process. The plugin that faulted is never called again:
 * the frames between the fault and this call are abandoned, so its state is suspect.
 *
 * - Windows (MSVC): structured exception handling, __try/__except. C++ exceptions (which
 *   include Rust panics, code 0xE06D7363) are not caught here; they keep unwinding to the
 *   caller's catch_unwind.
 * - Unix: SIGSEGV, SIGBUS, SIGILL and SIGFPE handlers, installed once by ph_guard_install,
 *   jump back with siglongjmp when the faulting thread is inside a guarded call and chain to
 *   the previous handler otherwise. sigsetjmp(..., 0) saves no signal mask, so a guarded call
 *   makes no system call (it runs on the audio thread); SA_NODEFER keeps the signal unblocked
 *   after the jump.
 * - Elsewhere (or a Windows compiler without SEH): the call is unguarded and
 *   ph_guard_supported() returns 0.
 */

typedef void (*ph_guard_fn)(void *ctx);

#if defined(_MSC_VER)

#include <windows.h>

static int ph_filter(unsigned long code, unsigned long *out) {
    switch (code) {
    case EXCEPTION_ACCESS_VIOLATION:
    case EXCEPTION_ILLEGAL_INSTRUCTION:
    case EXCEPTION_PRIV_INSTRUCTION:
    case EXCEPTION_INT_DIVIDE_BY_ZERO:
    case EXCEPTION_INT_OVERFLOW:
    case EXCEPTION_ARRAY_BOUNDS_EXCEEDED:
    case EXCEPTION_DATATYPE_MISALIGNMENT:
    case EXCEPTION_IN_PAGE_ERROR:
    case EXCEPTION_FLT_DIVIDE_BY_ZERO:
    case EXCEPTION_FLT_INVALID_OPERATION:
        *out = code;
        return EXCEPTION_EXECUTE_HANDLER;
    default:
        return EXCEPTION_CONTINUE_SEARCH;
    }
}

int ph_guard_supported(void) { return 1; }

void ph_guard_install(void) {}

int ph_guard_call(ph_guard_fn f, void *ctx, unsigned long *code) {
    unsigned long c = 0;
    __try {
        f(ctx);
    } __except (ph_filter(GetExceptionCode(), &c)) {
        *code = c;
        return 1;
    }
    return 0;
}

#elif defined(__unix__) || defined(__APPLE__)

#include <pthread.h>
#include <setjmp.h>
#include <signal.h>
#include <string.h>

static __thread sigjmp_buf *ph_jump;
static __thread volatile int ph_signal;
static struct sigaction ph_previous[4];
static const int ph_signals[4] = {SIGSEGV, SIGBUS, SIGILL, SIGFPE};
static pthread_once_t ph_once = PTHREAD_ONCE_INIT;

static void ph_handler(int sig, siginfo_t *info, void *uc) {
    sigjmp_buf *j = ph_jump;
    if (j) {
        ph_jump = 0;
        ph_signal = sig;
        siglongjmp(*j, 1);
    }
    /* not ours: restore the previous handler and let the fault happen again under it */
    for (int i = 0; i < 4; i++) {
        if (ph_signals[i] == sig) {
            sigaction(sig, &ph_previous[i], 0);
            if ((ph_previous[i].sa_flags & SA_SIGINFO) && ph_previous[i].sa_sigaction) {
                ph_previous[i].sa_sigaction(sig, info, uc);
            }
            return;
        }
    }
}

static void ph_install_once(void) {
    for (int i = 0; i < 4; i++) {
        struct sigaction sa;
        memset(&sa, 0, sizeof sa);
        sa.sa_sigaction = ph_handler;
        sa.sa_flags = SA_SIGINFO | SA_NODEFER | SA_ONSTACK;
        sigemptyset(&sa.sa_mask);
        sigaction(ph_signals[i], &sa, &ph_previous[i]);
    }
}

int ph_guard_supported(void) { return 1; }

void ph_guard_install(void) { pthread_once(&ph_once, ph_install_once); }

int ph_guard_call(ph_guard_fn f, void *ctx, unsigned long *code) {
    sigjmp_buf buf;
    sigjmp_buf *previous = ph_jump;
    if (sigsetjmp(buf, 0) == 0) {
        ph_jump = &buf;
        f(ctx);
        ph_jump = previous;
        return 0;
    }
    ph_jump = previous;
    *code = (unsigned long)ph_signal;
    return 1;
}

#else

int ph_guard_supported(void) { return 0; }

void ph_guard_install(void) {}

int ph_guard_call(ph_guard_fn f, void *ctx, unsigned long *code) {
    (void)code;
    f(ctx);
    return 0;
}

#endif
