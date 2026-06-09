use crate::semantic_facts::InternalRange;

pub(super) fn find_line_containing(source: &str, needle: &str) -> Option<usize> {
    source
        .lines()
        .enumerate()
        .find_map(|(index, line)| line.contains(needle).then_some(index))
}

pub(super) fn line_range(
    source: &str,
    line_index: usize,
    start_column: usize,
    end_column: usize,
) -> InternalRange {
    let line_len = line_len(source, line_index);
    InternalRange {
        start_line: line_index as u32,
        start_column: start_column.min(line_len) as u32,
        end_line: line_index as u32,
        end_column: end_column.min(line_len) as u32,
    }
}

pub(super) fn whole_file_range(source: &str) -> InternalRange {
    let line_count = source.lines().count();
    if line_count == 0 {
        return InternalRange {
            start_line: 0,
            start_column: 0,
            end_line: 0,
            end_column: 0,
        };
    }
    let end_line = line_count - 1;
    InternalRange {
        start_line: 0,
        start_column: 0,
        end_line: end_line as u32,
        end_column: line_len(source, end_line) as u32,
    }
}

pub(super) fn line_len(source: &str, line_index: usize) -> usize {
    source.lines().nth(line_index).map(str::len).unwrap_or(0)
}

pub(super) fn leading_spaces(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}
