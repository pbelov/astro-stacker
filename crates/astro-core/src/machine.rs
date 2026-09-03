//! What the machine this is running on can be asked about itself.
//!
//! Two questions, and both are asked because the alternative is a constant that
//! is wrong in both directions. A budget sized for a workstation makes a laptop
//! swap; one sized for a laptop leaves a workstation reading its frames twice.
//!
//! Every answer is a fact about the machine and never about the frames. What to
//! do with it belongs to the pass that asks.

/// How many cores the machine really has, or `None` where it will not say.
///
/// Not [`std::thread::available_parallelism`], which answers a different
/// question: it reports logical parallelism, and on a part with simultaneous
/// multithreading that is twice the cores. The distinction is measured rather
/// than aesthetic — on this project's reference session a pass that decodes
/// frames ran materially faster on the physical count than on the logical one,
/// because the second thread on a core brings no decoder of its own and shares
/// the cache and the memory pipe with the first.
///
/// This used to be the logical count halved, which is the same answer on a part
/// with two threads per core and wrong on one without.
pub fn physical_cores() -> Option<usize> {
    sysinfo::System::physical_core_count().filter(|count| *count > 0)
}

/// How much physical memory the machine has, in bytes.
///
/// Total and deliberately not available. Free memory is the more useful number
/// and the wrong one: it is a fact about the moment rather than about the
/// machine, it falls as a run proceeds and every finished master stays resident,
/// and a budget cut from it would send two identically sized sets down different
/// paths depending on which was built first, or on what else was open. A
/// combination that took the median on Tuesday and the clipped mean on Wednesday
/// is not a pipeline anybody can reason about.
///
/// So this is the machine's specification, which two runs of the same command
/// agree about. What guards against actually exhausting memory is the share
/// taken of it and the fact that the pass models its own peak — see
/// [`crate::calibrate::combine`].
pub fn total_bytes() -> Option<u64> {
    let mut system = sysinfo::System::new();
    system.refresh_memory();
    let total = system.total_memory();
    (total > 0).then_some(total)
}
