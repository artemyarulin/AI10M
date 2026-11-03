// Subtract N from N² N times, check for zero, and print elapsed wall time.
#include <stdint.h>
#include <stdio.h>
#include <time.h>

static int do_cpu_calculations(void) {
    const uint64_t n = 191692582605ULL; // Change this to tune the duration.
    volatile uint64_t result = n * n; // Keep the compiler from removing the work.
    for (uint64_t i = n; i > 0; --i)
        result -= n;
    return result != 0;
}

int main(void) {
    time_t start = time(NULL);
    int failed = do_cpu_calculations();
    printf("Completed in %.0f seconds\n", difftime(time(NULL), start));
    return failed;
}
