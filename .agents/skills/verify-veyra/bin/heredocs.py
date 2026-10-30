import re

HEREDOC = re.compile(r"<<'(?P<end>PY[A-Z_]*)'\n(?P<body>.*?)\n(?P=end)(?:\n|$)",re.S)
