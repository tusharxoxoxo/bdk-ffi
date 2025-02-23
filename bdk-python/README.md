# bdk-python
The Python language bindings for the [bitcoindevkit](https://github.com/bitcoindevkit).

See the [package on PyPI](https://pypi.org/project/bdkpython/).  

## Install from PyPI
Install the latest release using
```shell
pip install bdkpython
```

## Run the tests
```shell
pip install --requirement requirements.txt
bash ./generate.sh
python setup.py bdist_wheel --verbose
pip install ./dist/bdkpython-<yourversion>-py3-none-any.whl --force-reinstall
python -m unittest --verbose tests/test_bdk.py
```

## Build the package
```shell
# Install dependencies
pip install --requirement requirements.txt

# Generate the bindings
bash generate.sh

# Build the wheel
python setup.py --verbose bdist_wheel
```

## Run tox to build and test locally
```shell
# install dev requirements
pip install --requirement requirements-dev.txt

# build bindings glue code (located at ./src/bdkpython/bdk.py)
source ./generate.sh

# build and test
tox -vv
```

## Install locally
```shell
pip install ./dist/bdkpython-<yourversion>-py3-none-any.whl
```
