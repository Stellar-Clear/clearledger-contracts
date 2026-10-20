import re
with open("contracts/invoice/src/test.rs", "r") as f:
    text = f.read()

text = re.sub(
    r"<<<<<<< HEAD\n([\s\S]*?)=======\n([\s\S]*?)>>>>>>> origin/main",
    r"\1\n\2",
    text
)
with open("contracts/invoice/src/test.rs", "w") as f:
    f.write(text)

