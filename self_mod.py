#!/usr/bin/env python3
"""
Self-modifying script: reads its own source, appends a new line,
and writes the modified source back to itself.
"""

import os

def main():
    script_path = os.path.abspath(__file__)
    
    # Read the current source
    with open(script_path, 'r') as f:
        source = f.read()
    
    # The new line to append
    new_line = "\n# Self-modified on " + __import__('datetime').strftime('%Y-%m-%d %H:%M:%S') + "\n"
    
    # Append the new line
    modified_source = source + new_line
    
    # Write back to the same file
    with open(script_path, 'w') as f:
        f.write(modified_source)
    
    print(f"Self-modified {script_path}")
    print(f"Added: {new_line.strip()}")

if __name__ == "__main__":
    main()
