//! Ruby 4.0.7's public instance methods of the core containers, `Object`'s and `Kernel`'s among them, as
//! `Class#public_instance_methods` lists them: what an instance of a subclass answers without `method_missing`.

pub(crate) const HASH: &[&str] = &[
    "!", "!=", "!~", "<", "<=", "<=>", "==", "===",
    ">", ">=", "[]", "[]=", "__id__", "__send__", "all?", "any?",
    "assoc", "chain", "chunk", "chunk_while", "class", "clear", "clone", "collect",
    "collect_concat", "compact", "compact!", "compare_by_identity", "compare_by_identity?", "count", "cycle", "deconstruct_keys",
    "default", "default=", "default_proc", "default_proc=", "define_singleton_method", "delete", "delete_if", "detect",
    "dig", "display", "drop", "drop_while", "dup", "each", "each_cons", "each_entry",
    "each_key", "each_pair", "each_slice", "each_value", "each_with_index", "each_with_object", "empty?", "entries",
    "enum_for", "eql?", "equal?", "except", "extend", "fetch", "fetch_values", "filter",
    "filter!", "filter_map", "find", "find_all", "find_index", "first", "flat_map", "flatten",
    "freeze", "frozen?", "grep", "grep_v", "group_by", "has_key?", "has_value?", "hash",
    "include?", "inject", "inspect", "instance_eval", "instance_exec", "instance_of?", "instance_variable_defined?", "instance_variable_get",
    "instance_variable_set", "instance_variables", "invert", "is_a?", "itself", "keep_if", "key", "key?",
    "keys", "kind_of?", "lazy", "length", "map", "max", "max_by", "member?",
    "merge", "merge!", "method", "methods", "min", "min_by", "minmax", "minmax_by",
    "nil?", "none?", "object_id", "one?", "partition", "private_methods", "protected_methods", "public_method",
    "public_methods", "public_send", "rassoc", "reduce", "rehash", "reject", "reject!", "remove_instance_variable",
    "replace", "respond_to?", "reverse_each", "select", "select!", "send", "shift", "singleton_class",
    "singleton_method", "singleton_methods", "size", "slice", "slice_after", "slice_before", "slice_when", "sort",
    "sort_by", "store", "sum", "take", "take_while", "tally", "tap", "then",
    "to_a", "to_enum", "to_h", "to_hash", "to_proc", "to_s", "to_set", "transform_keys",
    "transform_keys!", "transform_values", "transform_values!", "uniq", "update", "value?", "values", "values_at",
    "yield_self", "zip",
];

pub(crate) const ARRAY: &[&str] = &[
    "!", "!=", "!~", "&", "*", "+", "-", "<<",
    "<=>", "==", "===", "[]", "[]=", "__id__", "__send__", "all?",
    "any?", "append", "assoc", "at", "bsearch", "bsearch_index", "chain", "chunk",
    "chunk_while", "class", "clear", "clone", "collect", "collect!", "collect_concat", "combination",
    "compact", "compact!", "concat", "count", "cycle", "deconstruct", "define_singleton_method", "delete",
    "delete_at", "delete_if", "detect", "difference", "dig", "display", "drop", "drop_while",
    "dup", "each", "each_cons", "each_entry", "each_index", "each_slice", "each_with_index", "each_with_object",
    "empty?", "entries", "enum_for", "eql?", "equal?", "extend", "fetch", "fetch_values",
    "fill", "filter", "filter!", "filter_map", "find", "find_all", "find_index", "first",
    "flat_map", "flatten", "flatten!", "freeze", "frozen?", "grep", "grep_v", "group_by",
    "hash", "include?", "index", "inject", "insert", "inspect", "instance_eval", "instance_exec",
    "instance_of?", "instance_variable_defined?", "instance_variable_get", "instance_variable_set", "instance_variables", "intersect?", "intersection", "is_a?",
    "itself", "join", "keep_if", "kind_of?", "last", "lazy", "length", "map",
    "map!", "max", "max_by", "member?", "method", "methods", "min", "min_by",
    "minmax", "minmax_by", "nil?", "none?", "object_id", "one?", "pack", "partition",
    "permutation", "pop", "prepend", "private_methods", "product", "protected_methods", "public_method", "public_methods",
    "public_send", "push", "rassoc", "reduce", "reject", "reject!", "remove_instance_variable", "repeated_combination",
    "repeated_permutation", "replace", "respond_to?", "reverse", "reverse!", "reverse_each", "rfind", "rindex",
    "rotate", "rotate!", "sample", "select", "select!", "send", "shift", "shuffle",
    "shuffle!", "singleton_class", "singleton_method", "singleton_methods", "size", "slice", "slice!", "slice_after",
    "slice_before", "slice_when", "sort", "sort!", "sort_by", "sort_by!", "sum", "take",
    "take_while", "tally", "tap", "then", "to_a", "to_ary", "to_enum", "to_h",
    "to_s", "to_set", "transpose", "union", "uniq", "uniq!", "unshift", "values_at",
    "yield_self", "zip", "|",
];

pub(crate) const STRING: &[&str] = &[
    "!", "!=", "!~", "%", "*", "+", "+@", "-@",
    "<", "<<", "<=", "<=>", "==", "===", "=~", ">",
    ">=", "[]", "[]=", "__id__", "__send__", "append_as_bytes", "ascii_only?", "b",
    "between?", "byteindex", "byterindex", "bytes", "bytesize", "byteslice", "bytesplice", "capitalize",
    "capitalize!", "casecmp", "casecmp?", "center", "chars", "chomp", "chomp!", "chop",
    "chop!", "chr", "clamp", "class", "clear", "clone", "codepoints", "concat",
    "count", "crypt", "dedup", "define_singleton_method", "delete", "delete!", "delete_prefix", "delete_prefix!",
    "delete_suffix", "delete_suffix!", "display", "downcase", "downcase!", "dump", "dup", "each_byte",
    "each_char", "each_codepoint", "each_grapheme_cluster", "each_line", "empty?", "encode", "encode!", "encoding",
    "end_with?", "enum_for", "eql?", "equal?", "extend", "force_encoding", "freeze", "frozen?",
    "getbyte", "grapheme_clusters", "gsub", "gsub!", "hash", "hex", "include?", "index",
    "insert", "inspect", "instance_eval", "instance_exec", "instance_of?", "instance_variable_defined?", "instance_variable_get", "instance_variable_set",
    "instance_variables", "intern", "is_a?", "itself", "kind_of?", "length", "lines", "ljust",
    "lstrip", "lstrip!", "match", "match?", "method", "methods", "next", "next!",
    "nil?", "object_id", "oct", "ord", "partition", "prepend", "private_methods", "protected_methods",
    "public_method", "public_methods", "public_send", "remove_instance_variable", "replace", "respond_to?", "reverse", "reverse!",
    "rindex", "rjust", "rpartition", "rstrip", "rstrip!", "scan", "scrub", "scrub!",
    "send", "setbyte", "singleton_class", "singleton_method", "singleton_methods", "size", "slice", "slice!",
    "split", "squeeze", "squeeze!", "start_with?", "strip", "strip!", "sub", "sub!",
    "succ", "succ!", "sum", "swapcase", "swapcase!", "tap", "then", "to_c",
    "to_enum", "to_f", "to_i", "to_r", "to_s", "to_str", "to_sym", "tr",
    "tr!", "tr_s", "tr_s!", "undump", "unicode_normalize", "unicode_normalize!", "unicode_normalized?", "unpack",
    "unpack1", "upcase", "upcase!", "upto", "valid_encoding?", "yield_self",
];

pub(crate) const OBJECT: &[&str] = &[
    "!", "!=", "!~", "<=>", "==", "===", "__id__", "__send__",
    "class", "clone", "define_singleton_method", "display", "dup", "enum_for", "eql?", "equal?",
    "extend", "freeze", "frozen?", "hash", "inspect", "instance_eval", "instance_exec", "instance_of?",
    "instance_variable_defined?", "instance_variable_get", "instance_variable_set", "instance_variables", "is_a?", "itself", "kind_of?", "method",
    "methods", "nil?", "object_id", "private_methods", "protected_methods", "public_method", "public_methods", "public_send",
    "remove_instance_variable", "respond_to?", "send", "singleton_class", "singleton_method", "singleton_methods", "tap", "then",
    "to_enum", "to_s", "yield_self",
];

/// `Enumerable`'s public instance methods: what a class including it answers without `method_missing`.
pub(crate) const ENUMERABLE: &[&str] = &[
    "all?", "any?", "chain", "chunk", "chunk_while", "collect", "collect_concat", "compact",
    "count", "cycle", "detect", "drop", "drop_while", "each_cons", "each_entry", "each_slice",
    "each_with_index", "each_with_object", "entries", "filter", "filter_map", "find", "find_all", "find_index",
    "first", "flat_map", "grep", "grep_v", "group_by", "include?", "inject", "lazy",
    "map", "max", "max_by", "member?", "min", "min_by", "minmax", "minmax_by",
    "none?", "one?", "partition", "reduce", "reject", "reverse_each", "select", "slice_after",
    "slice_before", "slice_when", "sort", "sort_by", "sum", "take", "take_while", "tally",
    "to_a", "to_h", "to_set", "uniq", "zip",
];

/// `Comparable`'s.
pub(crate) const COMPARABLE: &[&str] = &[
    "<", "<=", "==", ">", ">=", "between?", "clamp",
];

/// `Kernel`'s private instance methods: what a receiverless call answers without `method_missing` (`block_given?`).
pub(crate) const KERNEL_PRIVATE: &[&str] = &[
    "Array", "Complex", "Float", "Hash", "Integer", "Pathname", "Rational", "String",
    "__callee__", "__dir__", "__method__", "`", "abort", "at_exit", "autoload", "autoload?",
    "binding", "block_given?", "caller", "caller_locations", "catch", "eval", "exec", "exit",
    "exit!", "fail", "fork", "format", "gem", "gem_original_require", "gets", "global_variables",
    "initialize_clone", "initialize_copy", "initialize_dup", "instance_variables_to_inspect", "iterator?", "lambda", "load", "local_variables",
    "loop", "open", "p", "pp", "print", "printf", "proc", "putc",
    "puts", "raise", "rand", "readline", "readlines", "require", "require_relative", "respond_to_missing?",
    "select", "set_trace_func", "sleep", "spawn", "sprintf", "srand", "syscall", "system",
    "test", "throw", "trace_var", "trap", "untrace_var", "warn",
];
