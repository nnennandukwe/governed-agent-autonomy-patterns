"""Execute only the documented bounded arithmetic function, never arbitrary Python."""
import ast
import json
import math
import sys


def validate(source):
    """Validate and normalize the bounded arithmetic function before execution."""
    tree = ast.parse(source)
    if len(tree.body) != 1 or not isinstance(tree.body[0], ast.FunctionDef):
        raise ValueError('Provide exactly one shipping_quote function; imports and top-level execution are forbidden.')
    fn = tree.body[0]
    if fn.name != 'shipping_quote' or fn.decorator_list or fn.returns or fn.type_comment:
        raise ValueError('Only an undecorated shipping_quote function is allowed.')
    args = fn.args
    if ([a.arg for a in args.args] != ['quantity', 'unit_price'] or args.posonlyargs
            or args.vararg or args.kwarg or args.kwonlyargs or args.defaults
            or args.kw_defaults or any(a.annotation for a in args.args)):
        raise ValueError('Use shipping_quote(quantity, unit_price) without defaults or annotations.')
    nodes = list(ast.walk(tree))
    if len(nodes) > 300:
        raise ValueError('Function exceeds the 300-node execution limit.')
    allowed = (ast.Module, ast.FunctionDef, ast.arguments, ast.arg, ast.Return,
               ast.Assign, ast.AugAssign, ast.Name, ast.Load, ast.Store, ast.If,
               ast.Compare, ast.GtE, ast.Gt, ast.LtE, ast.Lt, ast.Eq, ast.NotEq,
               ast.BinOp, ast.Add, ast.Sub, ast.Mult, ast.Div, ast.UnaryOp,
               ast.USub, ast.UAdd, ast.Constant, ast.Raise, ast.Call)
    for node in nodes:
        if not isinstance(node, allowed):
            raise ValueError('Unsupported syntax: ' + type(node).__name__)
        if isinstance(node, ast.FunctionDef) and node is not fn:
            raise ValueError('Nested functions are forbidden.')
        if isinstance(node, ast.Name):
            if node.id.startswith('_') or len(node.id) > 40:
                raise ValueError('Invalid local name.')
            if isinstance(node.ctx, ast.Store) and node.id in ('round', 'ValueError', 'shipping_quote'):
                raise ValueError('Reserved names cannot be assigned.')
        if isinstance(node, ast.Constant):
            if isinstance(node.value, str):
                # Strings are permitted only as a literal ValueError message.
                if len(node.value) > 200 or not any(isinstance(p, ast.Call) and isinstance(p.func, ast.Name) and p.func.id == 'ValueError' and node in p.args for p in nodes):
                    raise ValueError('Strings are permitted only as short ValueError messages.')
            elif type(node.value) not in (int, float) or not math.isfinite(node.value) or abs(node.value) > 10000:
                raise ValueError('Only bounded numeric constants are allowed.')
        if isinstance(node, ast.Call):
            if not isinstance(node.func, ast.Name) or node.func.id not in ('round', 'ValueError') or node.keywords:
                raise ValueError('Only round and ValueError calls are allowed.')
            if node.func.id == 'round' and not 1 <= len(node.args) <= 2:
                raise ValueError('round expects one or two arguments.')
            if node.func.id == 'ValueError' and (len(node.args) > 1 or not any(isinstance(p, ast.Raise) and p.exc is node for p in nodes)):
                raise ValueError('ValueError is allowed only in a raise statement.')
        if isinstance(node, ast.Raise) and (node.cause or not (isinstance(node.exc, ast.Call) or isinstance(node.exc, ast.Name) and node.exc.id == 'ValueError')):
            raise ValueError('Use raise ValueError with a literal message.')
    precision_constants = set()
    for node in nodes:
        if isinstance(node, ast.Call) and node.func.id == 'round' and len(node.args) == 2:
            precision = node.args[1]
            if not isinstance(precision, ast.Constant) or type(precision.value) is not int or not 0 <= precision.value <= 6:
                raise ValueError('Rounding precision must be a literal from zero to six.')
            precision_constants.add(id(precision))
    # Floating-point arithmetic has bounded storage; forbid unbounded integer growth.
    for node in nodes:
        if isinstance(node, ast.Constant) and type(node.value) in (int, float) and id(node) not in precision_constants:
            node.value = float(node.value)
    return ast.fix_missing_locations(tree)


try:
    payload = json.load(sys.stdin)
    tree = validate(payload['source'])
    namespace = {'__builtins__': {}, 'round': round, 'ValueError': ValueError}
    exec(compile(tree, '<governed-shipping-function>', 'exec'), namespace)
    observations = []
    for quantity, price in payload['inputs']:
        try:
            value = namespace['shipping_quote'](float(quantity), float(price))
            if type(value) not in (int, float) or not math.isfinite(value):
                observations.append({'error': 'non_finite_or_non_numeric_result'})
            else:
                observations.append({'value': value})
        except Exception as error:
            observations.append({'error': type(error).__name__})
    print(json.dumps({'observations': observations}))
except Exception as error:
    print(json.dumps({'validation_error': str(error)[:500]}))
