# circuits-game
This game is about simulating the internal circuts of a computer chip.
I am aiming for ridiclout levels of performance so that larger chips can be build and ran extremly quickly.


# GPU double buffer
This game runs its main logic on GPU since its anyway double buffered writes.
logically speaking we have 1 giant double buffer setup which we write individual into.

but because of GPU memory limitations we need to be slightly clever and split it into multiple buffers
and batch the writes into u32s so they dont have to be atomic.

each u32 in the write buffer has a single invocation worker assosited with it,
that worker goes ahead and runs all the relvent logic for figuring out a single non atomic write.
This means that it gets an ARRAY of commands which would be an index+len into a command buffer.
we may run out of room in the command buffer and need to run multiple shader invocations on diffrent buffers.

for comunication between buffers we write into INPUT/OUTPUT components which have their own subslice in the write buffer.
we generally want to group these components at the start of the buffer so cache behivior is nicer, but this isnt technically a hard requirment.

during the normal forward pass INPUT/OUTPUT bits get 0s written to them.
then AFTER this normal forward pass is complete we have the write buffer ready for reading.

to glue the INPUT/OUTPUT pairs we run a shader between the 2 write buffers. 
so we take the write buffer of OUTPUT and we copy the correct bits into INPUT 
using the same u32 batching idea from earlier and a basic bitwise OR.

This means we dont need atomics anywhere for anything!!!
and if we batch logical components to be generally on the same buffer we seriously limit cross buffer writes.

We benifit a lot from GPU having no memory coherence requirment here. because it means all the false shares which on CPU would be a huge issue are not a factor. instead the GPU should be able to put each u32 into the correct part of memory to be modified independently.

# Packing
we generally dont want to run a small component with 8ish elements as a full kernel invocation.
instead we try and allocate multiple components onto the same buffer+ops combo.
This also reduces the need for INPUT/OUTPUT gluing later on because local indecies do not need any gluing.

to allow this we record all connections in a logical plan which then gets compiled down before we actually run it on GPU.
this cost should be acceptable as its O(N) and we generally need to be able to run O(N) on every sinlge frame anyway.
so for now we compile from scratch every time.

there is some argument for how large to make the buffer, its probably best to choose as large a size as we can get away with.
altogh large buffers mean there is more risk of requiring more than 1 command buffers per charge buffer. and that can cause issues with sleeping work groups. So a moderately large is probably best for getting a smooth exprince.