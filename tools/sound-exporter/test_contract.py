"""Semantic checks for the recorder/exporter boundary, separate from inference."""
import json
from pathlib import Path
import tempfile
import unittest
from compare import compare, new_uncertainty
from export import empty_census, render
from oracle_ext import load

LAB=Path(__file__).resolve().parent.parent/'lab'


class Contract(unittest.TestCase):
    def setUp(self):
        self.oracle=load(LAB)

    def accepts(self,ty,value):
        grammar=self.oracle.Grammar('type selected = '+ty+'\n')
        graph=self.oracle.ValueGraph(value)
        return self.oracle.Membership(grammar,graph,'selected').accepted

    def test_bottom_is_empty_even_for_the_old_sentinel_hash(self):
        c=empty_census();ty=render({'kind':'bottom'},c)
        samples=[{'id':0,'tag':'nil'},{'id':0,'tag':'Integer','value':1},
            {'id':0,'tag':'Hash','entries':[{'key':{'id':1,'tag':'Symbol','value':'__bottom__'},
                'value':{'id':2,'tag':'nil'}}]}]
        self.assertTrue(all(not self.accepts(ty,v) for v in samples))

    def test_unknown_and_unsupported_are_counted_separately(self):
        rows=[({'kind':'var','var':17},'var'),
              ({'kind':'untyped','provenance':'pending'},'pending'),
              ({'kind':'untyped','provenance':'unresolved'},'unresolved'),
              ({'kind':'class','id':'ExternalWidget','args':[]},'unsupported')]
        for raw,category in rows:
            c=empty_census();ty=render(raw,c)
            self.assertTrue(c[category])
            self.assertEqual(sum(bool(v) for v in c.values()),1)
            self.assertTrue(self.accepts(ty,{'id':0,'tag':'nil'}))

    def test_new_opaque_slots_cannot_be_offset_by_improvements_elsewhere(self):
        delta=new_uncertainty({'unresolved':['new'],'unsupported':['shared']},
                              {'unresolved':['old'],'var':['shared']})
        self.assertEqual(delta['unresolved'],['new'])
        self.assertEqual(delta['unsupported'],['shared'])
        self.assertEqual(delta['var'],[])

    def test_time_stays_a_time(self):
        self.assertTrue(self.accepts('Time',{'id':0,'tag':'Time','value':'2026-10-10T15:00:00Z'}))
        self.assertFalse(self.accepts('String',{'id':0,'tag':'Time','value':'2026-10-10T15:00:00Z'}))
        with self.assertRaises(self.oracle.InputError):
            self.oracle.ValueGraph({'id':0,'tag':'Time'})

    def test_exception_snapshot_is_explicit_and_nominal(self):
        sample={'id':0,'tag':'Exception','class':'PublicError','message':'example',
                'ancestors':['PublicError','StandardError','Exception','Object']}
        self.assertTrue(self.accepts('untyped',sample))
        self.assertFalse(self.accepts('String',sample))
        with self.assertRaises(self.oracle.InputError):
            self.oracle.ValueGraph({'id':0,'tag':'Exception'})

    def test_missing_fails_and_unobserved_and_unbound_are_reported(self):
        with tempfile.TemporaryDirectory(dir=str(LAB.parent/'tmp')) as tmp:
            d=Path(tmp)
            (d/'selection.json').write_text(json.dumps({'missing':['X','f','return'],'unused':['X','g','return']}))
            categories={'missing':empty_census(),'unused':empty_census()}
            categories['missing']['missing']=['slot']
            (d/'categories.json').write_text(json.dumps(categories))
            for stage in ['final','graph']:(d/(stage+'.rbs')).write_text('type missing = never\ntype unused = untyped\n')
            records=[{'kind':'meta','format':'shape-oracle/v1'},
                {'kind':'value','slot':'known','value':{'id':0,'tag':'nil'}},
                {'kind':'value','slot':'ghost','value':{'id':0,'tag':'Integer','value':7}},
                {'kind':'complete','records':2}]
            trace=d/'trace.jsonl';trace.write_text(''.join(json.dumps(r)+'\n' for r in records))
            r=compare(LAB,trace,d,{'known':'missing','not_called':'unused'})
            self.assertEqual((r['records'],r['no_slot'],r['selected_slots'],r['observed_slots']),(2,1,2,1))
            self.assertEqual(r['arms']['final']['rejected'],1)
            self.assertEqual(r['arms']['final']['missing_observations'],1)

    def test_incomplete_and_failed_trace_are_input_errors(self):
        with tempfile.TemporaryDirectory(dir=str(LAB.parent/'tmp')) as tmp:
            trace=Path(tmp)/'trace.jsonl'
            for records in [[{'kind':'meta','format':'shape-oracle/v1'}],
                [{'kind':'meta','format':'shape-oracle/v1'},{'kind':'error','message':'test failed'}]]:
                trace.write_text(''.join(json.dumps(r)+'\n' for r in records))
                with self.assertRaises(self.oracle.InputError):self.oracle.load_records(trace)

    def test_matching_slot_labels_do_not_hide_a_different_recorded_source(self):
        with tempfile.TemporaryDirectory(dir=str(LAB.parent/'tmp')) as tmp:
            d=Path(tmp)
            (d/'selection.json').write_text(json.dumps({'selected':{
                'kind':'ivar_read','file':'app/jobs/base.rb','source_sha256':'static-source'}}))
            records=[{'kind':'meta','format':'shape-oracle/v1',
                'sources':[{'path':'app/jobs/base.rb','sha256':'different-runtime-source'}]},
                {'kind':'value','slot':'same-label','value':{'id':0,'tag':'nil'}},
                {'kind':'complete','records':1}]
            trace=d/'trace.jsonl';trace.write_text(''.join(json.dumps(r)+'\n' for r in records))
            with self.assertRaisesRegex(ValueError,'source hashes differ'):
                compare(LAB,trace,d,{'same-label':'selected'})


if __name__=='__main__':unittest.main()
