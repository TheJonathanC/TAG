use std::collections::VecDeque;

#[derive(Debug, Clone)]
pub struct Line {
    pub text: String,
}

impl Line {
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }
}

#[derive(Debug, Clone)]
pub struct ScrollbackRing {
    capacity: usize,
    buffer: VecDeque<Line>,
    total_added: usize,
}

impl ScrollbackRing {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            buffer: VecDeque::with_capacity(capacity.min(4096)),
            total_added: 0,
        }
    }

    pub fn push(&mut self, line: Line) {
        if self.buffer.len() >= self.capacity {
            self.buffer.pop_front();
        }
        self.buffer.push_back(line);
        self.total_added += 1;
    }

    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn get(&self, index: usize) -> Option<&Line> {
        self.buffer.get(index)
    }

    pub fn total_added(&self) -> usize {
        self.total_added
    }

    pub fn clear(&mut self) {
        self.buffer.clear();
        self.total_added = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ring_buffer_capacity() {
        let mut ring = ScrollbackRing::new(3);
        ring.push(Line::new("1"));
        ring.push(Line::new("2"));
        ring.push(Line::new("3"));
        assert_eq!(ring.len(), 3);
        assert_eq!(ring.get(0).unwrap().text, "1");

        ring.push(Line::new("4"));
        assert_eq!(ring.len(), 3);
        assert_eq!(ring.get(0).unwrap().text, "2");
        assert_eq!(ring.get(1).unwrap().text, "3");
        assert_eq!(ring.get(2).unwrap().text, "4");
        assert_eq!(ring.total_added(), 4);
    }
}
