# Run on a real JVM: jruby tests/support/jdbc_value_semantics.rb
# Requires JRuby 10+ and jdbc-sqlite3. ROUNDHOUSE_SOURCE is only for an
# external probe; once copied under tests/support the repository is inferred.
raise "JRuby required" unless RUBY_ENGINE == "jruby"

require "json"
source = ENV.fetch("ROUNDHOUSE_SOURCE") { File.expand_path("../..", __dir__) }
require File.join(source, "runtime/spinel/db_jruby")

module JdbcValueSemantics
  class << self
    attr_accessor :checks

    def equal(label, expected, actual)
      raise "#{label}: expected #{expected.inspect}, got #{actual.inspect}" unless expected == actual
      self.checks += 1
    end

    def truth(label, actual)
      equal(label, true, !!actual)
    end

    def rows(sql, binds = [], uncached: false)
      stmt = uncached ? Db.prepare_uncached(sql) : Db.prepare(sql)
      binds.each_with_index { |(method, value), index| Db.public_send(method, stmt, index + 1, value) }
      result = []
      while Db.step?(stmt)
        result << Array.new(Db.column_count(stmt)) { |index| Db.column_text_opt(stmt, index) }
      end
      result
    ensure
      Db.finalize(stmt) if stmt
    end

    def string_cases
      Db.exec("CREATE TABLE writer_values (id INTEGER PRIMARY KEY, value BLOB)")
      cases = [
        ["empty UTF-8", "", "text"],
        ["quoted ASCII UTF-8", "a'b\\c", "text"],
        ["multibyte UTF-8", "雪 café 😀", "text"],
        ["NUL UTF-8", "a\0雪", "blob"],
        ["empty BINARY", "".b, "text"],
        ["ASCII BINARY", "ascii ' \\".b, "text"],
        ["NUL BINARY", "a\0b".b, "blob"],
        ["non-ASCII BINARY", [0xff, 0x80, 0x61].pack("C*"), "blob"],
        ["UTF-8 bytes tagged BINARY", "雪 😀".b, "blob"],
        ["invalid UTF-8 with NUL", [0xff, 0x00, 0x80, 0x61].pack("C*").force_encoding(Encoding::UTF_8), "blob"]
      ]
      cases.each_with_index do |(label, value, kind), index|
        id = index + 1
        hex = value.unpack1("H*").upcase
        Db.exec("INSERT INTO writer_values VALUES (#{id}, #{Db.escape_string(value)})")
        equal("#{label}: writer storage and bytes", [[kind, hex]],
          rows("SELECT typeof(value), hex(value) FROM writer_values WHERE id = #{id}"))
        equal("#{label}: binder storage and bytes", [[kind, hex]],
          rows("SELECT typeof(?), hex(?)", [[:bind_text, value], [:bind_text, value]]))
        equal("#{label}: bound equality finds inline writer", [[id.to_s]],
          rows("SELECT id FROM writer_values WHERE id = #{id} AND value = ?", [[:bind_text, value]]))

        # Drop every reference to the input bytes before stepping. This
        # checks the actual JDBC value after mutation and collection.
        stmt = Db.prepare("SELECT typeof(?), hex(?)")
        input = value.dup
        Db.bind_text(stmt, 1, input)
        Db.bind_text(stmt, 2, input)
        input.replace("replaced")
        input = nil
        GC.start
        truth("#{label}: copied bind returns a row", Db.step?(stmt))
        equal("#{label}: copied bind type", kind, Db.column_text(stmt, 0))
        equal("#{label}: copied bind bytes", hex, Db.column_text(stmt, 1))
        Db.finalize(stmt)
        puts JSON.generate(case: label, storage_class: kind, byte_hex: hex, writer_binder_equal: true)
      end
    end

    def optional_cases
      [:bind_int_opt, :bind_text_opt, :bind_bool_opt].each do |method|
        truth("optional primitive #{method} exists", Db.respond_to?(method))
      end
      Db.exec("CREATE TABLE nullable_rows (id INTEGER PRIMARY KEY, number INTEGER, name TEXT, flag BOOLEAN)")
      cases = [[1, nil, nil, nil], [2, 0, "", false], [3, -17, "雪", true], [4, 2**40 + 123, "binary\0name".b, nil]]
      cases.each do |id, number, name, flag|
        Db.exec("INSERT INTO nullable_rows VALUES (#{id}, #{Db.escape_int_opt(number)}, #{Db.escape_string_opt(name)}, #{Db.escape_bool_opt(flag)})")
      end
      cached = {}
      [1, 3, 2, 4, 1, 4, 2, 3, 1].each do |id|
        _, number, name, flag = cases.fetch(id - 1)
        binds = []
        predicates = [["number", :bind_int_opt, number], ["name", :bind_text_opt, name],
                      ["flag", :bind_bool_opt, flag]].map do |column, method, value|
          if value.nil?
            "#{column} IS NULL"
          else
            binds << [method, value]
            "#{column} = ?"
          end
        end
        sql = "SELECT id FROM nullable_rows WHERE " + predicates.join(" AND ")
        stmt = Db.prepare(sql)
        truth("optional prepare is lazy", !stmt.executed)
        truth("optional values reuse their JDBC shape", cached[sql].equal?(stmt.pstmt)) if cached[sql]
        cached[sql] = stmt.pstmt
        equal("nil predicates remove slots", binds.length, sql.count("?"))
        binds.each_with_index { |(method, value), index| Db.public_send(method, stmt, index + 1, value) }
        truth("optional tuple #{id} matches", Db.step?(stmt))
        equal("optional tuple #{id} lockstep", id, Db.column_int(stmt, 0))
        truth("optional tuple #{id} exact cardinality", !Db.step?(stmt))
        Db.finalize(stmt)
      end
      equal("optional tuples have three null patterns", 3, cached.length)
      [[:bind_int_opt, nil, "null"], [:bind_int_opt, 0, "integer"],
       [:bind_text_opt, nil, "null"], [:bind_text_opt, "", "text"],
       [:bind_bool_opt, nil, "null"], [:bind_bool_opt, false, "integer"],
       [:bind_bool_opt, true, "integer"]].each do |method, value, kind|
        equal("#{method} #{value.inspect} storage", [[kind]], rows("SELECT typeof(?)", [[method, value]]))
      end
      [nil, false, true].each do |value|
        expected = value.nil? ? ["null", "-7"] : ["integer", value ? "1" : "0"]
        equal("nullable primitive bool #{value.inspect}", [expected],
          rows("SELECT typeof(?), COALESCE(?, -7)", [[:bind_bool, value], [:bind_bool, value]]))
      end
      puts "optional int/text/bool: NULL, zero, empty text, false, true, binary and 64-bit values pass"
    end

    def transient_cases
      Db.exec("CREATE TABLE in_rows (id INTEGER PRIMARY KEY)")
      (1..6).each { |id| Db.exec("INSERT INTO in_rows VALUES (#{id})") }
      conn = Db.current_dbh
      sql = "SELECT id FROM in_rows WHERE id IN (1, 2, 3) ORDER BY id"
      # Even if the same SQL already has a cached statement, the IN path
      # must create separate live handles and leave the cached one alone.
      cached_stmt = Db.prepare(sql)
      cached = cached_stmt.pstmt
      Db.finalize(cached_stmt)
      cache_size = conn.stmt_cache.size
      first = Db.prepare_uncached(sql)
      second = Db.prepare_uncached(sql)
      first_ps, second_ps = first.pstmt, second.pstmt
      truth("overlapping IN statements are distinct", !first_ps.equal?(second_ps))
      truth("uncached IN bypasses existing cache", !first_ps.equal?(cached) && !second_ps.equal?(cached))
      truth("first IN step", Db.step?(first))
      equal("first IN starts at one", 1, Db.column_int(first, 0))
      truth("second IN step", Db.step?(second))
      equal("second IN starts at one", 1, Db.column_int(second, 0))
      truth("first IN retains its cursor", Db.step?(first))
      equal("first IN advances independently", 2, Db.column_int(first, 0))
      Db.finalize(first)
      truth("first IN closes on finalize", first_ps.is_closed)
      truth("second IN stays open", !second_ps.is_closed)
      truth("second IN retains its cursor", Db.step?(second))
      equal("second IN advances independently", 2, Db.column_int(second, 0))
      Db.finalize(second)
      truth("second IN closes on finalize", second_ps.is_closed)
      truth("cached statement stays open", !cached.is_closed)
      equal("IN leaves statement cache unchanged", cache_size, conn.stmt_cache.size)
      [[1], [2, 5], [6, 3, 1], []].each do |ids|
        expected = ids.sort.map { |id| [id.to_s] }
        equal("IN cardinality #{ids.length}", expected,
          rows("SELECT id FROM in_rows WHERE id IN (#{Db.escape_int_list(ids)}) ORDER BY id", uncached: true))
      end
      equal("varying IN leaves statement cache unchanged", cache_size, conn.stmt_cache.size)

      Db.query_cache_begin
      begin
        first = Db.prepare_uncached(sql)
        original = first.pstmt
        truth("partial IN row", Db.step?(first))
        Db.finalize(first)
        truth("partial IN original closed", original.is_closed)
        second = Db.prepare_uncached(sql)
        truth("IN result replay has no JDBC statement", second.pstmt.nil?)
        truth("IN replay prefix", Db.step?(second))
        equal("IN replay prefix value", 1, Db.column_int(second, 0))
        truth("IN replay promotes", Db.step?(second))
        promoted = second.pstmt
        truth("promoted IN is a real statement", !promoted.nil?)
        equal("IN replay resumes after prefix", 2, Db.column_int(second, 0))
        truth("promoted IN remains transient", !second.cached)
        Db.finalize(second)
        truth("promoted IN closes", promoted.is_closed)
        equal("replay promotion leaves cache unchanged", cache_size, conn.stmt_cache.size)
      ensure
        Db.query_cache_end
      end
      equal("finalized IN leaves no live handles", 0, conn.open_statements.size)
      puts "IN: varying arity, overlap, existing-cache bypass, replay promotion and finalize cleanup pass"
    end

    def cleanup_cases
      owner = nil
      abandoned = []
      begin
        Db.with_connection do
          owner = Db.current_dbh
          2.times do
            stmt = Db.prepare_uncached("SELECT id FROM in_rows WHERE id IN (1, 2) ORDER BY id")
            Db.step?(stmt)
            abandoned << [stmt.pstmt, stmt.rs]
          end
          raise "expected unwind"
        end
      rescue RuntimeError => error
        raise unless error.message == "expected unwind"
      end
      abandoned.each do |ps, rs|
        truth("lease unwind closes IN PreparedStatement", ps.is_closed)
        truth("lease unwind closes IN ResultSet", rs.is_closed)
      end
      equal("lease unwind clears ownership", 0, owner.open_statements.size)
      outside = Db.prepare_uncached("SELECT id FROM in_rows WHERE id IN (2, 4)")
      Db.step?(outside)
      ps, rs, raw = outside.pstmt, outside.rs, Db.current_dbh.raw
      Db.close
      truth("Db.close closes out-of-lease IN statement", ps.is_closed)
      truth("Db.close closes out-of-lease result set", rs.is_closed)
      truth("Db.close closes JDBC connection", raw.is_closed)
      puts "IN cleanup: exception unwind and pool close pass with actual JDBC isClosed assertions"
    end

    def run
      self.checks = 0
      Db.configure(":memory:", pool_size: 1)
      puts "#{RUBY_DESCRIPTION}; SQLite #{rows('SELECT sqlite_version()').first.first}"
      Db.with_connection do
        string_cases
        optional_cases
        transient_cases
      end
      cleanup_cases
      puts JSON.generate(result: "PASS", assertions: checks)
    ensure
      Db.close
    end
  end
end

JdbcValueSemantics.run
