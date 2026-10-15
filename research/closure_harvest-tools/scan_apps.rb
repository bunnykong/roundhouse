# Static prediction for research condition on real apps (Prism, CRuby 4.0.7).
#
# Mirrors, on Prism's AST, the spine's ivar-harvest walker (`walk_ivar_assignments`,
# the legacy region) and the card's new region (C3's entrances: Array elements,
# Hash keys/values, keyword hashes, splat operands, ivar-assignment right sides).
# In the new region, variant E never enters a closure body; variant I enters a closure
# body exactly when its consumer applies it (the research condition rule):
#   A1 receiver of call/yield/[]/=== (literal, or lambda/proc/Proc.new with a block)
#   A2 block of a call (literal block or &literal), except closure constructors
#      (lambda, proc, Proc.new) and self-rebinding calls on another receiver
#   A3 positional or keyword argument of a call
#   A4 value of a local-variable binding
# Writes: @x =, @x ||= / &&= / op=, multi-assign ivar targets, @h[k] = / ||=,
# and self instance_variable_set with two arguments.
#
# Usage: ruby tools/scan_apps.rb NAME=DIR ... > results/scan.json
# Prints counts per app and every I-tag site (path:line, consumer). Public apps only.
require "json"
require "prism"
require "erb"

INVOKERS = %i[call yield [] ===].freeze
REBINDERS = %i[instance_exec instance_eval].freeze
ALWAYS_FOREIGN = %i[class_exec class_eval module_exec module_eval].freeze

class Scan
  attr_reader :counts, :sites

  def initialize(path)
    @path = path
    @counts = Hash.new(0)
    @sites = []
  end

  def closure_literal?(n)
    return true if n.is_a?(Prism::LambdaNode)
    return false unless n.is_a?(Prism::CallNode) && n.block.is_a?(Prism::BlockNode)
    constructor?(n)
  end

  def constructor?(call)
    (%i[lambda proc].include?(call.name) && call.receiver.nil?) ||
      (call.name == :new && call.receiver.is_a?(Prism::ConstantReadNode) && call.receiver.name == :Proc)
  end

  def foreign?(call)
    r = call.receiver
    return true if ALWAYS_FOREIGN.include?(call.name)
    return true if REBINDERS.include?(call.name) && !(r.nil? || r.is_a?(Prism::SelfNode))
    return true if call.name == :new && r.is_a?(Prism::ConstantReadNode) && %i[Class Module Struct].include?(r.name)
    return true if call.name == :define && r.is_a?(Prism::ConstantReadNode) && r.name == :Data
    false
  end

  def closure_body(n)
    n.is_a?(Prism::LambdaNode) ? n.body : n.block.body
  end

  def line(n)
    n.location.start_line
  end

  # region: :legacy, or [:new, variant, tag] where variant is :E or :I and tag is :E or :I
  def record(n, region, kind)
    if region == :legacy
      @counts["legacy_writes"] += 1
    else
      _, arm, tag = region
      @counts["#{arm}_new_#{tag}_writes"] += 1
      if (tag == :I && arm == :I) || (tag == :E && arm == :E)
        @sites << { path: @path, line: line(n), arm: arm, tag: tag, kind: kind, consumer: @consumer }
      end
    end
  end

  def reflective?(call)
    call.name == :instance_variable_set && (call.receiver.nil? || call.receiver.is_a?(Prism::SelfNode)) &&
      call.arguments && call.arguments.arguments.length == 2
  end

  def ivar_write?(n)
    n.is_a?(Prism::InstanceVariableWriteNode) || n.is_a?(Prism::InstanceVariableOrWriteNode) ||
      n.is_a?(Prism::InstanceVariableAndWriteNode) || n.is_a?(Prism::InstanceVariableOperatorWriteNode)
  end

  def args_of(call)
    call.arguments ? call.arguments.arguments : []
  end

  def walk_body(n, region)
    walk(n, region) if n
  end

  # Apply a closure in the new region under variant I.
  def apply(closure, region, consumer)
    _, arm, = region
    return if arm == :E
    saved = @consumer
    @consumer ||= consumer
    walk_body(closure_body(closure), [:new, :I, :I])
    @consumer = saved
  end

  def walk(n, region)
    case n
    when nil then nil
    when Prism::InstanceVariableWriteNode, Prism::InstanceVariableOrWriteNode,
         Prism::InstanceVariableAndWriteNode, Prism::InstanceVariableOperatorWriteNode
      record(n, region, "ivar_write")
      # The ivar right side is a C3 entrance.
      walk_new(n.value, region == :legacy ? [:new, nil, :E] : region)
    when Prism::MultiWriteNode
      (n.lefts + [n.rest, *n.rights]).compact.each do |t|
        record(t, region, "multi_write") if t.is_a?(Prism::InstanceVariableTargetNode)
      end
      walk(n.value, region)
    when Prism::LocalVariableWriteNode, Prism::LocalVariableOrWriteNode,
         Prism::LocalVariableAndWriteNode, Prism::LocalVariableOperatorWriteNode
      if region != :legacy && closure_literal?(n.value)
        apply(n.value, region, "A4 local")
      else
        walk(n.value, region)
      end
    when Prism::ConstantWriteNode, Prism::GlobalVariableWriteNode, Prism::ClassVariableWriteNode,
         Prism::ConstantOrWriteNode, Prism::ConstantPathWriteNode
      walk(n.value, region)
    when Prism::IndexOperatorWriteNode, Prism::IndexOrWriteNode, Prism::IndexAndWriteNode
      record(n, region, "index_write") if n.receiver.is_a?(Prism::InstanceVariableReadNode)
      walk(n.receiver, region)
      args_of(n).each { |a| walk(a, region) }
      walk(n.value, region)
    when Prism::CallNode then walk_call(n, region)
    when Prism::LambdaNode
      walk_body(n.body, region) if region == :legacy # new region: constructed, not run
    when Prism::BlockNode
      walk_body(n.body, region) if region == :legacy
    when Prism::StatementsNode then n.body.each { |c| walk(c, region) }
    when Prism::BeginNode
      walk_body(n.statements, region)
      r = n.rescue_clause
      while r
        walk_body(r.statements, region)
        r = r.respond_to?(:subsequent) ? r.subsequent : r.consequent
      end
      walk_body(n.else_clause&.statements, region)
      walk_body(n.ensure_clause&.statements, region)
    when Prism::ParenthesesNode then walk_body(n.body, region)
    when Prism::IfNode
      walk(n.predicate, region)
      walk_body(n.statements, region)
      sub = n.respond_to?(:subsequent) ? n.subsequent : n.consequent
      walk(sub, region)
    when Prism::UnlessNode
      walk(n.predicate, region)
      walk_body(n.statements, region)
      walk(n.respond_to?(:else_clause) ? n.else_clause : n.consequent, region)
    when Prism::ElseNode then walk_body(n.statements, region)
    when Prism::WhileNode, Prism::UntilNode
      walk(n.predicate, region)
      walk_body(n.statements, region)
    when Prism::CaseNode
      n.conditions.each { |w| walk_body(w.statements, region) }
      walk(n.respond_to?(:else_clause) ? n.else_clause : n.consequent, region)
    when Prism::AndNode, Prism::OrNode
      walk(n.left, region)
      walk(n.right, region)
    when Prism::RescueModifierNode
      walk(n.expression, region)
      walk(n.rescue_expression, region)
    when Prism::ReturnNode
      args_of(n).each { |a| walk(a, region) }
    when Prism::ArrayNode
      n.elements.each { |e| walk_new(e, region == :legacy ? [:new, nil, :E] : region) }
    when Prism::HashNode, Prism::KeywordHashNode
      r = region == :legacy ? [:new, nil, :E] : region
      n.elements.each do |el|
        if el.is_a?(Prism::AssocNode)
          walk_new(el.key, r)
          if n.is_a?(Prism::KeywordHashNode) && closure_literal?(el.value)
            each_arm(r) { |rr| apply(el.value, rr, "A3 keyword argument") }
          else
            walk_new(el.value, r)
          end
        elsif el.is_a?(Prism::AssocSplatNode)
          walk_new(el.value, r)
        end
      end
    when Prism::SplatNode
      walk_new(n.expression, region == :legacy ? [:new, nil, :E] : region)
    when Prism::AssocSplatNode
      walk_new(n.value, region == :legacy ? [:new, nil, :E] : region)
    end
  end

  # Entering the new region from the legacy walker forks into both variants.
  def walk_new(n, region)
    if region[1].nil?
      walk(n, [:new, :E, :E])
      walk(n, [:new, :I, :E])
    else
      walk(n, region)
    end
  end

  def each_arm(region)
    if region[1].nil?
      yield [:new, :E, :E]
      yield [:new, :I, :E]
    else
      yield region
    end
  end

  def walk_call(n, region)
    record(n, region, "instance_variable_set") if reflective?(n)
    if n.name == :[]= && n.receiver.is_a?(Prism::InstanceVariableReadNode)
      record(n, region, "index_send")
    end
    if region == :legacy
      walk(n.receiver, region)
      args_of(n).each { |a| walk(a, region) }
      b = n.block
      if b.is_a?(Prism::BlockNode) then walk_body(b.body, region)
      elsif b.is_a?(Prism::BlockArgumentNode) then walk(b.expression, region)
      end
      return
    end
    # New region.
    if n.receiver && closure_literal?(n.receiver) && INVOKERS.include?(n.name)
      apply(n.receiver, region, "A1 #{n.name}")
    else
      walk(n.receiver, region)
    end
    args_of(n).each do |a|
      if closure_literal?(a)
        apply(a, region, "A3 argument")
      else
        walk(a, region)
      end
    end
    b = n.block
    if b.is_a?(Prism::BlockNode)
      apply(n, region, "A2 block of #{n.name}") unless constructor?(n) || foreign?(n)
    elsif b.is_a?(Prism::BlockArgumentNode)
      if b.expression && closure_literal?(b.expression)
        apply(b.expression, region, "A2 &literal to #{n.name}") unless foreign?(n)
      else
        walk(b.expression, region)
      end
    end
  end
end

def bodies(src, erb)
  src = ERB.new(src, trim_mode: "-").src if erb
  result = Prism.parse(src)
  return [] if result.failure?
  return [result.value] if erb
  out = []
  stack = [result.value]
  until stack.empty?
    n = stack.pop
    if n.is_a?(Prism::DefNode)
      out << n.body if n.body
    else
      stack.concat(n.compact_child_nodes)
    end
  end
  out
end

report = {}
all_sites = []
ARGV.each do |arg|
  name, dir = arg.split("=", 2)
  totals = Hash.new(0)
  roots = ENV.fetch("CLOSURE_SCAN_ROOTS", "app,lib")
  files = Dir.glob(File.join(dir, "{#{roots}}", "**", "*.{rb,erb}")).sort
  files.each do |f|
    rel = f.delete_prefix(dir + "/")
    begin
      bodies(File.read(f), f.end_with?(".erb")).each do |body|
        s = Scan.new("#{name}/#{rel}")
        s.walk(body, :legacy)
        s.counts.each { |k, v| totals[k] += v }
        all_sites.concat(s.sites)
      end
    rescue StandardError => e
      totals["errors"] += 1
      warn "#{rel}: #{e.class}"
    end
  end
  totals["files"] = files.length
  mine = all_sites.select { |s| s[:path].start_with?("#{name}/") }
  totals["I_sites"] = mine.count { |s| s[:tag] == :I }
  totals["E_sites"] = mine.count { |s| s[:tag] == :E }
  report[name] = totals.sort.to_h
end
puts JSON.pretty_generate(ruby: RUBY_DESCRIPTION, prism: Prism::VERSION, apps: report, sites: all_sites)
