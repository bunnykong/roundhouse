"""Use the unchanged membership algorithm, with Time and true empty type atoms."""
import importlib.util
from pathlib import Path
import sys


def load(lab):
    spec = importlib.util.spec_from_file_location('sound_oracle', Path(lab) / 'oracle/check.py')
    checker = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = checker
    spec.loader.exec_module(checker)
    checker.ATOMS = checker.ATOMS | {'Time', 'never'}
    checker.TAGS = checker.TAGS | {'Time', 'Exception'}
    original_graph = checker.ValueGraph
    class AppValueGraph(original_graph):
        def __init__(self, root):
            super().__init__(root)
            for node in self.nodes.values():
                if node['tag']=='Time' and not isinstance(node.get('value'),str):
                    raise checker.InputError('Time needs an ISO timestamp payload')
                if node['tag']=='Exception' and (not isinstance(node.get('class'),str)
                    or not isinstance(node.get('message'),str)
                    or not isinstance(node.get('ancestors'),list)
                    or any(not isinstance(c,str) for c in node['ancestors'])):
                    raise checker.InputError('Exception needs class, message and nominal ancestors')
    checker.ValueGraph = AppValueGraph
    return checker
