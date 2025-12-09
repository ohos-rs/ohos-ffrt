#!/usr/bin/env python3
"""
Add #[cfg(feature = "api-XX")] attributes based on @since XX annotations in doc comments.
Only adds feature gates for API versions > BASELINE_API_VERSION.
Also handles enum constants that reference types with higher API versions.
"""

import re
import sys
from collections import OrderedDict
from typing import Set, Dict, Optional, List, Tuple


# Baseline API version - versions <= this don't need feature gates
BASELINE_API_VERSION = 12


def add_feature_gates(content: str) -> Tuple[str, Set[int]]:
    """
    Adds #[cfg(feature = "api-XX")] attributes based on @since annotations.
    Returns the processed content and a set of API versions found (> baseline).
    """
    since_re = re.compile(r'@since\s+(\d+)')
    type_def_re = re.compile(r'^pub type (\w+)\s*=')
    # Match single-line const: pub const NAME: TYPE = or pub const NAME : TYPE =
    const_single_line_re = re.compile(r'^pub const \w+\s*:\s*(\w+)\s*=')
    # Match start of multi-line const
    const_start_re = re.compile(r'^pub const \w+\s*:\s*$')
    # Match continuation line with type
    const_type_line_re = re.compile(r'^\s*(\w+)\s*=')
    
    lines = content.split('\n')
    api_versions: Set[int] = set()
    
    # First pass: collect all type definitions and their API versions
    type_versions: Dict[str, int] = {}
    pending_version: Optional[int] = None
    
    for line in lines:
        trimmed = line.strip()
        
        if trimmed.startswith('#[doc = '):
            # Extract @since version from doc comment
            match = since_re.search(trimmed)
            if match:
                version = int(match.group(1))
                if version > BASELINE_API_VERSION:
                    pending_version = version
        elif trimmed.startswith('pub type '):
            # Record type with its API version
            match = type_def_re.match(trimmed)
            if match and pending_version is not None:
                type_versions[match.group(1)] = pending_version
            pending_version = None
        elif not trimmed.startswith('#[') and trimmed:
            # Reset pending version for non-attribute, non-empty lines
            pending_version = None
    
    # Second pass: add cfg attributes
    result: List[str] = []
    cfg_already_added = False
    pending_const_line: Optional[str] = None
    pending_const_indent = ''
    
    for line in lines:
        trimmed = line.strip()
        indent = len(line) - len(line.lstrip())
        indent_str = line[:indent]
        
        # Handle continuation of multi-line const
        if pending_const_line is not None:
            match = const_type_line_re.match(trimmed)
            if match:
                type_name = match.group(1)
                if not cfg_already_added and type_name in type_versions:
                    version = type_versions[type_name]
                    api_versions.add(version)
                    result.append(f'{pending_const_indent}#[cfg(feature = "api-{version}")]')
                result.append(pending_const_line)
                cfg_already_added = False
            else:
                # Not a type line, just push the pending const line
                result.append(pending_const_line)
            
            pending_const_line = None
            pending_const_indent = ''
            result.append(line)
            continue
        
        if trimmed.startswith('#[doc = '):
            # Check for @since in doc comment
            match = since_re.search(trimmed)
            if match:
                version = int(match.group(1))
                if version > BASELINE_API_VERSION:
                    api_versions.add(version)
                    # Add cfg before the doc comment
                    result.append(f'{indent_str}#[cfg(feature = "api-{version}")]')
                    cfg_already_added = True
            result.append(line)
        elif trimmed.startswith('pub const '):
            # Check if this is a single-line or multi-line const
            if const_start_re.match(trimmed):
                # Multi-line const: buffer this line and wait for the type on next line
                pending_const_line = line
                pending_const_indent = indent_str
            else:
                match = const_single_line_re.match(trimmed)
                if match:
                    # Single-line const
                    if not cfg_already_added:
                        type_name = match.group(1)
                        if type_name in type_versions:
                            version = type_versions[type_name]
                            api_versions.add(version)
                            result.append(f'{indent_str}#[cfg(feature = "api-{version}")]')
                cfg_already_added = False
                result.append(line)
        elif (trimmed.startswith('pub type ')
              or trimmed.startswith('pub fn ')
              or trimmed.startswith('pub struct ')
              or trimmed.startswith('pub enum ')
              or trimmed.startswith('pub static ')):
            # Reset state for other declarations
            cfg_already_added = False
            result.append(line)
        elif (not trimmed.startswith('#[')
              and trimmed
              and not trimmed.startswith('//')
              and not trimmed.startswith('extern ')
              and not trimmed.startswith('{')
              and not trimmed.startswith('}')):
            # Reset state for other non-attribute lines
            cfg_already_added = False
            result.append(line)
        else:
            result.append(line)
    
    # Handle any remaining pending const line
    if pending_const_line is not None:
        result.append(pending_const_line)
    
    return '\n'.join(result), api_versions

def main():
    if len(sys.argv) < 1:
        print(f"Usage: {sys.argv[0]} <lib.rs>", file=sys.stderr)
        sys.exit(1)
    
    lib_rs_path = sys.argv[1]
    
    try:
        # Read the generated content
        with open(lib_rs_path, 'r', encoding='utf-8') as f:
            content = f.read()
        
        # Add feature gates based on @since annotations
        processed_content, api_versions = add_feature_gates(content)
        
        # Write the processed content back
        with open(lib_rs_path, 'w', encoding='utf-8') as f:
            f.write(processed_content)
        
        if api_versions:
            print(f"Found API versions: {sorted(api_versions)}")
            
        else:
            print("No API versions > 12 found")
        
        print(f"Successfully processed {lib_rs_path}")
    except Exception as e:
        print(f"Error: {e}", file=sys.stderr)
        import traceback
        traceback.print_exc()
        sys.exit(1)


if __name__ == '__main__':
    main()

