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

/// Physical memory not currently spoken for, in bytes.
///
/// Available and not total: what matters to a pass deciding whether to hold its
/// frames is what it can take without pushing something else out, and a machine
/// with a browser open has less of it than its specification says.
///
/// It follows that this moves between runs, so nothing that changes an *answer*
/// may depend on it — only how the answer is arrived at. See
/// [`crate::calibrate::combine`], where that distinction decides where the
/// budget's floor is.
pub fn available_bytes() -> Option<u64> {
    let mut system = sysinfo::System::new();
    system.refresh_memory();
    let available = system.available_memory();
    (available > 0).then_some(available)
}
