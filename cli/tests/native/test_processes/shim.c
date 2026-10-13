// A test process with a memory error and a race, standing in front of the
// one `buri test` started: it makes both, notes that it ran in the file its
// first argument names, then becomes the test process the rest name.

#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>

static int shared;

static void *other(void *unused) {
    (void)unused;
    shared++;
    return NULL;
}

int main(int argc, char **argv) {
    if (argc < 3) {
        return 2;
    }
    // A branch on a value never written: Memcheck's error.
    int *never = malloc(sizeof *never);
    if (*never == 42) {
        shared = 1;
    }
    free(never);
    // Two threads write `shared` with no lock between them: Helgrind's.
    pthread_t thread;
    pthread_create(&thread, NULL, other, NULL);
    shared++;
    pthread_join(thread, NULL);
    FILE *ran = fopen(argv[1], "a");
    if (ran == NULL) {
        return 2;
    }
    fputs("ran\n", ran);
    fclose(ran);
    execv(argv[2], argv + 2);
    return 127;
}
