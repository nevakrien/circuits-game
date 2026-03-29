# circuits-game
This game is about simulating the internal circuts of a computer chip.
I am aiming for ridiclout levels of performance so that larger chips can be build and ran extremly quickly.


# Software Design
for circuts to not care about insertion and excution order the only way to do an update is a double buffer. 
where we only read from previous steps and write to the current one. update happens explictly in these ticks,
which is very parallalizem friednly

Now there is a question of when/how we go about runing each circuts update rule.
supporting some sort of sleep can potentialy be very helpful. BUT we still have to render sleeping circuts.
so relying on sleep still forces us to run a render step which means we cant use it as agressively.

This observation naturally leads us to want the circuts data to already be ready for render, 
without forcing us to construct a render graph every tick. And that kind of implies a GPU based setup.
Which works very well with double-buffers (no false share risk).

when you add to this the fact most human built circuts follow a natural 2D structure it pushes us to want to store things closely based on their 2D distance to eachother, to have the best cache locality possible.

Each circut board is simulated as a grid where each cell has a charge and we run a kernel over that space to write the charge to the next buffer.
we also support having multiple sub component, these grids are allocated in standard sizes of Nx(N+-4) N<=1024 arenas which can each be a single large 3D texture subdivides properly into diffrent types of pages containing texture of thier type so each 1024x1024 slice can be split into multiple smaller slice.

circut are stored in a similar manner with a tag+data union aproch.
each location has 1 circut which it stores, this is chosen over a sparse represntation because the index would be 32bit (10x10x12) which is a lot of memory for this setup. for a CPU based aproch we would go dense and eat the cost.
the reason for using tags is that tag based scans have better cache locality which for this mostly memory bound setup is going to matter a lot.

since we are on GPU we can render directly from the grid texture and we can even render subcomponents directly without needing additional setup from CPU since the info about which grid subsecsion to render is stored at each subcompoenent. 
This should allow extremly smooth togeling on/off of subcomponent rendering as there is no addional graphs to be constructed.
