import re
with open("contracts/invoice/src/lib.rs", "r") as f:
    text = f.read()

text = re.sub(
    r"<<<<<<< HEAD\n([\s\S]*?)=======\n([\s\S]*?)>>>>>>> origin/main",
    r"\1\2",
    text
)
with open("contracts/invoice/src/lib.rs", "w") as f:
    f.write(text)

