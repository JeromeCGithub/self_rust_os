"""Evaluate the performance runner's actual launcher clicks against a reordered DE."""
import ast
from pathlib import Path
import unittest

ROOT=Path(__file__).resolve().parents[2]
class PerformanceLaunchers(unittest.TestCase):
    def test_performance_tasks_launch_the_named_applications_after_browser_install(self):
        tree=ast.parse((ROOT/'e2e/desktop_perf.py').read_text())
        points=[]
        for node in sorted(ast.walk(tree),key=lambda node:getattr(node,'lineno',0)):
            if not (isinstance(node,ast.Call) and isinstance(node.func,ast.Name) and node.func.id=='click' and len(node.args)==2):continue
            if not (isinstance(node.args[1],ast.Constant) and node.args[1].value==65):continue
            code=compile(ast.Expression(node.args[0]),'<launcher expression>','eval')
            points.append(eval(code,{'disk':'private.img','launchers':lambda _: {'PORTAL':450,'SNAKE':550,'SPIN':650}}))
        self.assertEqual(points,[450,550,650], 'Terminal, Snake and Spin must follow current DE discovery positions')
