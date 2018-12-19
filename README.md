# juliex - a simple futures executor

juliex is a concurrent executor for Rust futures. It is implemented as a
threadpool executor using a single, shared queue. Algorithmically, it is very
similar to the Threadpool executor provided by the futures crate. The main
difference is that juliex uses a crossbeam channel and performs a single
allocation per spawned future, whereas the futures Threadpool uses std
concurrency primitives and multiple allocations.

Similar to [romio][romio] - an IO reactor - juliex currently provides no user
configuration. It exposes only the simplest API possible.

[romio]: https://github.com/withoutboats/romio
