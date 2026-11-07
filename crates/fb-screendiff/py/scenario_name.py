"""Print the screen scenario's initial company name for the shell driver."""
import json
import sys

with open(sys.argv[1]) as source:
    print(json.load(source)['steps'][0]['name'])
