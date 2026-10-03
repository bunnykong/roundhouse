# Native ownership checks: count actual SQLite statements, including ones
# accidentally lost from the cache. This FFI hook is test-only.
module SQL
  ffi_func :sqlite3_next_stmt, [:ptr, :ptr], :ptr
end

def native_statement_count(conn)
  count = 0
  ptr = SQL.sqlite3_next_stmt(conn.dbh, nil)
  while !ptr.nil?
    count += 1
    ptr = SQL.sqlite3_next_stmt(conn.dbh, ptr)
  end
  count
end

Db.with_connection do
  conn = Db.current_conn
  outer = Db.prepare("SELECT ? AS transient_ownership")
  Db.bind_int(outer, 1, 71)
  before = native_statement_count(conn)
  inner = Db.prepare("SELECT ? AS transient_ownership")
  expect_int("busy hit prepares a transient", before + 1, native_statement_count(conn))
  Db.finalize(inner)
  expect_int("transient is really finalized", before, native_statement_count(conn))
  Db.finalize(outer)
end

# A latent SQLite execution error makes reset report a failure. The lease
# must still release every sibling, destroy the failed cached statement,
# and preserve its request error. Call FFI step directly so this covers
# release independently of the checked-step production path.
cleanup_conn = nil
before_cleanup = 0
begin
  Db.with_connection do
    cleanup_conn = Db.current_conn
    before_cleanup = native_statement_count(cleanup_conn)
    bad = Db.prepare("SELECT abs(-9223372036854775808) AS cleanup_native_failure")
    rc = SQL.sqlite3_step(bad)
    raise "missing native reset failure" if rc == SQL::ROW || rc == SQL::DONE
    a = Db.prepare("SELECT column1 FROM (VALUES (1), (2)) AS cleanup_native_sibling")
    b = Db.prepare("SELECT column1 FROM (VALUES (1), (2)) AS cleanup_native_sibling")
    Db.step?(a)
    Db.step?(b)
    raise "native request failed before cleanup"
  end
rescue RuntimeError => e
  raise e if e.message != "native request failed before cleanup"
end
expect_int("native failed cache entry and transient sibling were finalized", before_cleanup + 1, native_statement_count(cleanup_conn))
Db.with_connection do
  stmt = Db.prepare("SELECT 29 AS cleanup_native_recovery")
  raise "native cleanup made the lease unusable" if !Db.step?(stmt)
  expect_int("native cleanup recovery", 29, Db.column_int(stmt, 0))
  Db.finalize(stmt)
end
puts "runtime: native reset failure drains siblings and preserves request errors passed"

# trim! must preserve a cursor even if explicitly called mid-lease.
Db.with_connection do
  conn = Db.current_conn
  outer = Db.prepare("SELECT column1 FROM (VALUES (1), (2), (3)) WHERE column1 >= ?")
  Db.bind_int(outer, 1, 1)
  raise "missing trim seed" if !Db.step?(outer)
  i = 0
  while i < 140
    stmt = Db.prepare("SELECT " + i.to_s + " AS trim_pressure")
    Db.finalize(stmt)
    i += 1
  end
  conn.trim!
  raise "trim closed a live cursor" if !Db.step?(outer)
  expect_int("trim preserves cursor", 2, Db.column_int(outer, 0))
  Db.finalize(outer)
end

# Exceptional leases must also trim; cleanup includes busy-hit transients.
begin
  Db.with_connection do
    i = 0
    while i < 140
      stmt = Db.prepare("SELECT " + i.to_s + " AS exception_pressure")
      Db.finalize(stmt)
      i += 1
    end
    abandoned = Db.prepare("SELECT ? AS abandoned_pressure")
    nested = Db.prepare("SELECT ? AS abandoned_pressure")
    raise "pressure failed"
  end
rescue RuntimeError => e
  raise e if e.message != "pressure failed"
end
Db.with_connection do
  conn = Db.current_conn
  raise "exception bypassed trim" if native_statement_count(conn) > DbConn::CAP
  outer = Db.prepare("SELECT ? AS shutdown_ownership")
  inner = Db.prepare("SELECT ? AS shutdown_ownership")
  conn.finalize_all
  expect_int("shutdown finalizes cached and transient statements", 0, native_statement_count(conn))
end
puts "runtime: native transient release, trimming and shutdown passed"
