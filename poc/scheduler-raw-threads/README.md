# Userland scheduler

A minimal ARM Linux preemptive scheduler. One worker runs five number crunchers, switching between their contexts on a 2ms timer. The context switcher is specialized to these guest, no stack supported.

You can read more at https://kapteko.com/userland-linux-scheduler-in-rust/.