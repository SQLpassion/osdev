#ifndef SPINLOCK_H
#define SPINLOCK_H

// Declares a Spinlock
#define DECLARE_SPINLOCK(name) volatile int name ## Locked

// Acquires a Spinlock
#define AcquireSpinlock(name) \
    while (!__sync_bool_compare_and_swap(& name ## Locked, 0, 1)); \
    __sync_synchronize();

// Releases a Spinlock
#define ReleaseSpinlock(name) \
    __sync_synchronize(); \
    name ## Locked = 0;

#endif