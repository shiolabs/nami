import 'package:flutter/material.dart';

// A window with something drawn in it (`flutter create`'s runner, this app).
void main() => runApp(const MaterialApp(
      title: 'flutter-hello',
      home: Scaffold(
        backgroundColor: Color(0xFF2A6FC8),
        body: Center(child: Text('Hello from a bundle', style: TextStyle(color: Colors.white, fontSize: 32))),
      ),
    ));
