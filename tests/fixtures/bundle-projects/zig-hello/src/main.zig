const std = @import("std");

pub fn main() void {
    std.debug.print("zig-hello 0.2.0: hello, from Zig {s}\n", .{@import("builtin").zig_version_string});
}
