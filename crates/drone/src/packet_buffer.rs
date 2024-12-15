use std::collections::{HashMap, VecDeque};
use std::fmt;
use wg_2024::packet::Fragment;

/// A buffer to store and manage fragments for packet transmission.
///
/// The `PacketBuffer` maintains an ordered collection of fragments, allowing efficient
/// addition, retrieval, and removal of fragments. It ensures that older fragments are
/// automatically evicted when the buffer reaches its maximum capacity.
#[derive(Clone, Debug)]
pub struct PacketBuffer {
    /// Stores the fragments using `(session_id, fragment_index)` as the key.
    buffer: HashMap<(u64, u64), Fragment>,
    /// Maintains the insertion order of keys for efficient eviction of old fragments.
    order: VecDeque<(u64, u64)>,
    /// Maximum capacity of the buffer.
    max_size: usize,
}

impl PacketBuffer {
    /// Creates a new `PacketBuffer` with the specified maximum size.
    ///
    /// # Parameters
    ///
    /// - `max_size`: The maximum number of fragments the buffer can hold.
    ///
    /// # Returns
    ///
    /// A new `PacketBuffer` instance.
    pub fn new(max_size: usize) -> Self {
        Self {
            buffer: HashMap::new(),
            order: VecDeque::new(),
            max_size,
        }
    }

    /// Adds a fragment to the buffer.
    ///
    /// If the buffer is full, the oldest fragment is removed to make space for the new one.
    ///
    /// # Parameters
    ///
    /// - `session_id`: The session ID associated with the fragment.
    /// - `fragment`: The fragment to add.
    pub fn add_fragment(&mut self, session_id: u64, fragment: Fragment) {
        let key = (session_id, fragment.fragment_index);

        // If the buffer is full, remove the oldest fragment.
        if self.buffer.len() >= self.max_size {
            if let Some(oldest) = self.order.pop_front() {
                self.buffer.remove(&oldest);
            }
        }

        // Add the new fragment.
        self.buffer.insert(key, fragment);
        self.order.push_back(key);
    }

    /// Retrieves and removes a fragment from the buffer.
    ///
    /// # Parameters
    ///
    /// - `session_id`: The session ID associated with the fragment.
    /// - `fragment_index`: The index of the fragment.
    ///
    /// # Returns
    ///
    /// - `Some(Fragment)`: The fragment if found in the buffer.
    /// - `None`: If the fragment is not found.
    pub fn get_fragment(&mut self, session_id: u64, fragment_index: u64) -> Option<Fragment> {
        let key = (session_id, fragment_index);
        // Remove the fragment from the HashMap.
        if let Some(fragment) = self.buffer.remove(&key) {
            // Remove the key from the VecDeque.
            if let Some(pos) = self.order.iter().position(|&k| k == key) {
                self.order.remove(pos);
            }
            // Return the removed fragment.
            return Some(fragment);
        }
        None
    }

    /// Updates the maximum size of the buffer.
    ///
    /// # Parameters
    ///
    /// - `new_size`: The new maximum size of the buffer.
    pub fn edit_max_size_buffer(&mut self, new_size: usize) {
        if new_size > 1024 {
            self.max_size = 1024;
        } else {
            self.max_size = new_size;
        }
    }
}

impl fmt::Display for PacketBuffer {
    /// Provides a human-readable representation of the buffer's contents.
    ///
    /// The output includes the buffer's maximum size, current size, and details of each fragment.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "PacketBuffer (max_size: {}, current_size: {}):",
            self.max_size,
            self.buffer.len()
        )?;

        for (key, fragment) in &self.buffer {
            let (session_id, fragment_index) = key;
            writeln!(
                f,
                "  Session ID: {}, Fragment Index: {}, Total Fragments: {}, Length: {}",
                session_id, fragment_index, fragment.total_n_fragments, fragment.length
            )?;
        }

        Ok(())
    }
}
