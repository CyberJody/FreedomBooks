"""Screen fixtures are successful service calls, independently checked on both backends."""
import json
import sys
dump=json.load(open(sys.argv[1]))
bad=[row for row in dump['outcomes'] if row['ok'] is not True or row.get('unexpected')]
assert not bad, bad
print(f"Python fixture: {len(dump['outcomes'])} successful service steps")
