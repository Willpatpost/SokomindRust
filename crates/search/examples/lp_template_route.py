#!/usr/bin/env python3
"""LP-template route builder for Grand Hall.

This script constructs a route using the LP-guided paths from the plan.
The plan provides per-box paths (sequences of edges) that the LP found optimal.
"""

import json

RT = "/home/coder/.qwen/tmp/route-study"
WIDTH = 15
SIDES = 4

# Load the board
rows = None
for entry in json.load(open("/home/coder/SokomindRust/data/puzzles.json")):
    if entry["id"] == "huge":
        rows = entry["rows"]
        break

# Parse board geometry
walls = set()
boxes = {}
keeper = None
goals = {}
for r, row in enumerate(rows):
    for c, ch in enumerate(row):
        cell = r * WIDTH + c
        if ch == "O":
            walls.add(cell)
        elif ch == "R":
            keeper = cell
        elif ch.isupper() and ch not in "ORS":
            boxes[cell] = ch
        elif ch.islower() or ch == "S":
            goals[cell] = ch

print("Grand Hall geometry:")
print(f"  Keeper at {keeper}")
print(f"  Boxes at {sorted(boxes.keys())}")
print(f"  Goals at {sorted(goals.keys())}")

# Load edges
edges = []
for line in open(f"{RT}/edges.txt"):
    e, frm, to, end, stand, cost = (int(t) for t in line.split())
    edges.append((frm, to, end, stand, cost))

# Load the plan
plan = json.load(open(f"{RT}/plan.json"))

# Get start spots for each box
start_spots = {}
for line in open(f"{RT}/board.txt"):
    parts = line.split()
    if parts and parts[0] == "boxes":
        for p in parts[1:]:
            b, cell = p.split(":")
            start_spots[int(b)] = int(cell)

print(f"\nPlan loaded: {len(plan)} boxes")

# For each box, follow its plan edges to build the route
# The plan gives edges in sequence for each box

# First, let's understand the edge structure:
# edge index, from_spot, to_spot, end_cell, stand_cell, cost
# The "spot" is cell * SIDES + direction

def spot_to_cell(spot):
    return spot // SIDES

def spot_to_direction(spot):
    # 0=U, 1=D, 2=L, 3=R
    return spot % SIDES

print("\n" + "="*60)
print("Constructing route from LP template")
print("="*60)

# Build the route by following each box's edges
# We need to interleave moves from different boxes

# The plan's edges are ordered: the box follows its edges in sequence
# We need to find a valid interleaving that respects all constraints

# For simplicity, let's just simulate each box's path and collect moves
# This will give us the structure, then we can refine the interleaving

route = []
box_positions = dict(boxes)
keeper_pos = keeper

for box_id, box_plan in sorted(plan.items()):
    print(f"\nBox {box_id}:")
    print(f"  Start: {start_spots[box_id]}")
    print(f"  Goal: {box_plan['goal']}")
    print(f"  Edges: {box_plan['edges']}")
    
    # For each edge in the plan, simulate the move
    current_pos = start_spots[box_id]
    for edge_idx in box_plan['edges']:
        edge = edges[edge_idx]
        frm, to, end, stand, cost = edge
        frm_cell, to_cell = frm // SIDES, to // SIDES
        end_cell = end // SIDES
        stand_cell = stand // SIDES
        
        print(f"  Edge {edge_idx}: {frm_cell} -> {to_cell} (end {end_cell}, stand {stand_cell})")
        
        # The box moves from current_pos to to_cell
        # We need to figure out how the keeper gets to stand_cell
        # and pushes the box to to_cell
        
        # This is a simplified simulation - a full implementation
        # would need to track the keeper's position and path

print("\nNote: Full route construction requires:")
print("  1. Interleaving moves from all boxes")
print("  2. Tracking keeper path between pushes")
print("  3. Handling path conflicts and deadlocks")
print("\nThe LP template gives us the edge sequences per box.")
print("The next step is to find a valid interleaving using:")
print("  - The staged plan's keeper transportation")
print("  - The refined plan's greedy nearest-stand selection")
